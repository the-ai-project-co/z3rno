//! Python bindings for `z3rno-engine`, built with PyO3 + maturin.
//!
//! Sync-only surface: internally every engine call runs on one shared
//! Tokio runtime (`runtime()`, below) via `.block_on(...)`, so Python
//! callers never see `async`/`await` — `pip install z3rno` and a plain
//! `python -c "..."` one-liner is the whole story, no event-loop ceremony.
//!
//! `Memory`/`AuditEvent`/`ForgetProof` and `metadata` all cross into Python
//! via `pythonize` (`Serialize` -> Python object) rather than hand-written
//! `#[pyclass]` wrapper types for every response shape.

use std::sync::{Arc, OnceLock};

use pyo3::exceptions::{PyLookupError, PyPermissionError, PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pythonize::{depythonize, pythonize};
use tokio::runtime::Runtime;
use uuid::Uuid;
use z3rno_engine::backend::{BackendError, ErrorCode};
use z3rno_engine::lessons::{
    render_for_agent, Actor, LessonDraft, LessonFilter, LessonQuery, LessonStatus, Transition,
};
use z3rno_engine::{BackendTier, MemoryEngine, Tier};

/// The shared Tokio runtime every engine call is driven through. Built
/// lazily on first use, reused for the life of the process.
fn runtime() -> &'static Runtime {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| Runtime::new().expect("failed to start z3rno's Tokio runtime"))
}

/// Parses a snake_case tier string (as accepted by `Client.store`) into
/// `Tier`. Plain Rust in/out on purpose — it's exercised by an ordinary
/// `#[test]` below with no Python interpreter involved, sidestepping the
/// `extension-module` / `cargo test` linking conflict.
fn parse_tier(tier: &str) -> Result<Tier, String> {
    match tier {
        "working" => Ok(Tier::Working),
        "episodic" => Ok(Tier::Episodic),
        "semantic" => Ok(Tier::Semantic),
        "procedural" => Ok(Tier::Procedural),
        other => Err(format!(
            "invalid tier {other:?}: expected one of \"working\", \"episodic\", \"semantic\", \"procedural\""
        )),
    }
}

/// The string `Client.tier()` reports to Python for a given `BackendTier`.
fn tier_to_str(tier: BackendTier) -> &'static str {
    match tier {
        BackendTier::Embedded => "embedded",
        BackendTier::Postgres => "postgres",
    }
}

/// Parses `Client.store`'s `links` argument (`list[tuple[str, str]]` on the
/// Python side) into the engine's `Vec<(Uuid, String)>`.
fn parse_links(links: Vec<(String, String)>) -> Result<Vec<(Uuid, String)>, String> {
    links
        .into_iter()
        .map(|(id, relationship)| {
            Uuid::parse_str(&id)
                .map(|uuid| (uuid, relationship))
                .map_err(|e| format!("invalid link target id {id:?}: {e}"))
        })
        .collect()
}

/// Maps a `BackendError` to the Python exception it should surface as,
/// keeping the original error detail rather than swallowing it.
fn backend_error_to_pyerr(err: BackendError) -> PyErr {
    match err {
        BackendError::NotFound(msg) => PyLookupError::new_err(msg),
        BackendError::Storage(err) => PyRuntimeError::new_err(err.to_string()),
        err @ BackendError::ForgetIncomplete { .. } => PyRuntimeError::new_err(err.to_string()),
        err @ BackendError::Rejected {
            code: ErrorCode::NotPermitted,
            ..
        } => PyPermissionError::new_err(err.to_string()),
        err @ BackendError::Rejected { .. } => PyValueError::new_err(err.to_string()),
    }
}

/// `client.advanced.audit(...)` — the one advanced operation the engine
/// has (decision-doc 0003's two-tier `store`/`recall`/`forget` + advanced
/// pattern). No `.admin`/`.conversations` namespaces: the engine has
/// nothing behind those, so none are built here.
#[pyclass]
struct Advanced {
    engine: Arc<MemoryEngine>,
}

#[pymethods]
impl Advanced {
    /// The tenant's full audit chain, oldest first, as a list of dicts.
    fn audit(&self, py: Python<'_>, tenant_id: &str) -> PyResult<Py<PyAny>> {
        let engine = Arc::clone(&self.engine);
        let tenant_id = tenant_id.to_string();
        let events = py
            .detach(|| runtime().block_on(engine.advanced().audit(&tenant_id)))
            .map_err(backend_error_to_pyerr)?;
        Ok(pythonize(py, &events)?.unbind())
    }
}

/// `z3rno.Client()` — the embedded, zero-external-services default; pass
/// `backend="postgres", connection_string="postgres://..."` for the
/// production backend (0006.4).
#[pyclass]
struct Client {
    engine: Arc<MemoryEngine>,
}

#[pymethods]
impl Client {
    #[new]
    #[pyo3(signature = (backend="embedded", path="z3rno.db", connection_string=None))]
    fn new(backend: &str, path: &str, connection_string: Option<&str>) -> PyResult<Self> {
        let engine = match backend {
            "embedded" => MemoryEngine::embedded(path).map_err(backend_error_to_pyerr)?,
            "postgres" => {
                let connection_string = connection_string.ok_or_else(|| {
                    PyValueError::new_err(
                        "backend=\"postgres\" requires connection_string=\"postgres://...\"",
                    )
                })?;
                runtime()
                    .block_on(MemoryEngine::postgres(connection_string))
                    .map_err(backend_error_to_pyerr)?
            }
            other => {
                return Err(PyValueError::new_err(format!(
                    "invalid backend {other:?}: expected \"embedded\" or \"postgres\""
                )))
            }
        };
        Ok(Self {
            engine: Arc::new(engine),
        })
    }

    /// `"embedded"` or `"postgres"` — which tenant-isolation guarantee this
    /// client's backend provides.
    fn tier(&self) -> &'static str {
        tier_to_str(self.engine.tier())
    }

    /// `client.advanced.audit(...)` — see [`Advanced`].
    #[getter]
    fn advanced(&self) -> Advanced {
        Advanced {
            engine: Arc::clone(&self.engine),
        }
    }

    /// `client.lessons` — see [`Lessons`].
    #[getter]
    fn lessons(&self) -> Lessons {
        Lessons {
            engine: Arc::clone(&self.engine),
        }
    }

    /// Stores a memory, returning it as a dict with
    /// id/tenant_id/tier/content/metadata/created_at.
    #[pyo3(signature = (tenant_id, tier, content, embedding=None, metadata=None, links=vec![]))]
    #[allow(clippy::too_many_arguments)]
    fn store(
        &self,
        py: Python<'_>,
        tenant_id: &str,
        tier: &str,
        content: String,
        embedding: Option<Vec<f32>>,
        metadata: Option<Bound<'_, PyAny>>,
        links: Vec<(String, String)>,
    ) -> PyResult<Py<PyAny>> {
        let tier = parse_tier(tier).map_err(PyValueError::new_err)?;
        let metadata = match metadata {
            Some(obj) => depythonize(&obj)?,
            None => serde_json::Value::Object(serde_json::Map::new()),
        };
        let links = parse_links(links).map_err(PyValueError::new_err)?;
        let tenant_id = tenant_id.to_string();
        let engine = Arc::clone(&self.engine);

        let memory = py
            .detach(|| {
                runtime()
                    .block_on(engine.store(&tenant_id, tier, content, embedding, metadata, links))
            })
            .map_err(backend_error_to_pyerr)?;
        Ok(pythonize(py, &memory)?.unbind())
    }

    /// Recalls up to `k` memories most similar to `query`, as a list of
    /// dicts shaped like `store`'s return value.
    fn recall(
        &self,
        py: Python<'_>,
        tenant_id: &str,
        query: Vec<f32>,
        k: usize,
    ) -> PyResult<Py<PyAny>> {
        let tenant_id = tenant_id.to_string();
        let engine = Arc::clone(&self.engine);

        let memories = py
            .detach(|| runtime().block_on(engine.recall(&tenant_id, query, k)))
            .map_err(backend_error_to_pyerr)?;
        Ok(pythonize(py, &memories)?.unbind())
    }

    /// Forgets a memory, returning `None` if it was already gone, else a
    /// dict with audit_event_id/hash as an audit reference for the deletion.
    fn forget(&self, py: Python<'_>, tenant_id: &str, id: &str) -> PyResult<Py<PyAny>> {
        let id = Uuid::parse_str(id)
            .map_err(|e| PyValueError::new_err(format!("invalid id {id:?}: {e}")))?;
        let tenant_id = tenant_id.to_string();
        let engine = Arc::clone(&self.engine);

        let proof = py
            .detach(|| runtime().block_on(engine.forget(&tenant_id, id)))
            .map_err(backend_error_to_pyerr)?;
        match proof {
            Some(proof) => Ok(pythonize(py, &proof)?.unbind()),
            None => Ok(py.None()),
        }
    }
}

fn parse_id(id: &str) -> PyResult<Uuid> {
    Uuid::parse_str(id).map_err(|e| PyValueError::new_err(format!("invalid id {id:?}: {e}")))
}

/// `client.lessons` — scoped, human-gated lessons. Every write takes an
/// `actor` dict (`{"kind": "human"|"agent"|"ci", "name": ..., "session":
/// ...}`); the engine trusts it, so an interface an agent drives must only
/// ever pass `kind="agent"` (agents can only create candidates).
/// Drafts, queries and results are plain dicts matching the engine's
/// serde shapes.
#[pyclass]
struct Lessons {
    engine: Arc<MemoryEngine>,
}

/// Drives one engine future to completion with the GIL released.
fn block<T: Send>(
    py: Python<'_>,
    fut: impl std::future::Future<Output = Result<T, BackendError>> + Send,
) -> PyResult<T> {
    py.detach(|| runtime().block_on(fut))
        .map_err(backend_error_to_pyerr)
}

#[pymethods]
impl Lessons {
    /// Redacts and validates `draft` without storing it; returns what
    /// `create` would store.
    fn prepare(
        &self,
        py: Python<'_>,
        tenant_id: &str,
        draft: Bound<'_, PyAny>,
    ) -> PyResult<Py<PyAny>> {
        let draft: LessonDraft = depythonize(&draft)?;
        let prepared = self
            .engine
            .lessons(tenant_id)
            .prepare(draft)
            .map_err(backend_error_to_pyerr)?;
        Ok(pythonize(py, &prepared)?.unbind())
    }

    /// The text the engine embeds for `draft`, so callers embed lessons
    /// in the same space as the engine without re-implementing it.
    #[staticmethod]
    fn searchable_text(draft: Bound<'_, PyAny>) -> PyResult<String> {
        let draft: LessonDraft = depythonize(&draft)?;
        Ok(draft.searchable_text())
    }

    /// Stores a new lesson (`status` "candidate" or, for a human,
    /// "confirmed") and returns it.
    #[pyo3(signature = (tenant_id, actor, draft, status="candidate", embedding=None))]
    fn create(
        &self,
        py: Python<'_>,
        tenant_id: &str,
        actor: Bound<'_, PyAny>,
        draft: Bound<'_, PyAny>,
        status: &str,
        embedding: Option<Vec<f32>>,
    ) -> PyResult<Py<PyAny>> {
        let actor: Actor = depythonize(&actor)?;
        let draft: LessonDraft = depythonize(&draft)?;
        let status: LessonStatus = status.parse().map_err(backend_error_to_pyerr)?;
        let tenant_id = tenant_id.to_string();
        let engine = Arc::clone(&self.engine);
        let lesson = block(py, async move {
            let e = &engine;
            e.lessons(&tenant_id)
                .create(&actor, draft, status, embedding)
                .await
        })?;
        Ok(pythonize(py, &lesson)?.unbind())
    }

    /// The lesson with `id`, or `None`.
    fn get(&self, py: Python<'_>, tenant_id: &str, id: &str) -> PyResult<Py<PyAny>> {
        let id = parse_id(id)?;
        let tenant_id = tenant_id.to_string();
        let engine = Arc::clone(&self.engine);
        let lesson = block(py, async move {
            let e = &engine;
            e.lessons(&tenant_id).get(id).await
        })?;
        Ok(pythonize(py, &lesson)?.unbind())
    }

    /// Lessons, optionally filtered by status and repo.
    #[pyo3(signature = (tenant_id, status=None, repo=None))]
    fn list(
        &self,
        py: Python<'_>,
        tenant_id: &str,
        status: Option<&str>,
        repo: Option<String>,
    ) -> PyResult<Py<PyAny>> {
        let status = status
            .map(str::parse::<LessonStatus>)
            .transpose()
            .map_err(backend_error_to_pyerr)?;
        let tenant_id = tenant_id.to_string();
        let engine = Arc::clone(&self.engine);
        let lessons = block(py, async move {
            let e = &engine;
            e.lessons(&tenant_id)
                .list(&LessonFilter { status, repo })
                .await
        })?;
        Ok(pythonize(py, &lessons)?.unbind())
    }

    /// Applies "confirm", "reject", "retire" or "reinstate" (human only).
    #[pyo3(signature = (tenant_id, actor, id, transition, note=None))]
    fn transition(
        &self,
        py: Python<'_>,
        tenant_id: &str,
        actor: Bound<'_, PyAny>,
        id: &str,
        transition: &str,
        note: Option<String>,
    ) -> PyResult<Py<PyAny>> {
        let actor: Actor = depythonize(&actor)?;
        let transition: Transition = serde_json::from_value(serde_json::Value::from(transition))
            .map_err(|_| {
                PyValueError::new_err(format!(
                    "invalid transition {transition:?}: expected confirm, reject, retire or reinstate"
                ))
            })?;
        let id = parse_id(id)?;
        let tenant_id = tenant_id.to_string();
        let engine = Arc::clone(&self.engine);
        let lesson = block(py, async move {
            let e = &engine;
            e.lessons(&tenant_id)
                .transition(&actor, id, transition, note)
                .await
        })?;
        Ok(pythonize(py, &lesson)?.unbind())
    }

    /// Deletes a lesson (human only); `None` if there was no such lesson.
    fn delete(
        &self,
        py: Python<'_>,
        tenant_id: &str,
        actor: Bound<'_, PyAny>,
        id: &str,
    ) -> PyResult<Py<PyAny>> {
        let actor: Actor = depythonize(&actor)?;
        let id = parse_id(id)?;
        let tenant_id = tenant_id.to_string();
        let engine = Arc::clone(&self.engine);
        let proof = block(py, async move {
            let e = &engine;
            e.lessons(&tenant_id).delete(&actor, id).await
        })?;
        Ok(pythonize(py, &proof)?.unbind())
    }

    /// The lesson's review history, oldest first.
    fn history(&self, py: Python<'_>, tenant_id: &str, id: &str) -> PyResult<Py<PyAny>> {
        let id = parse_id(id)?;
        let tenant_id = tenant_id.to_string();
        let engine = Arc::clone(&self.engine);
        let events = block(py, async move {
            let e = &engine;
            e.lessons(&tenant_id).history(id).await
        })?;
        Ok(pythonize(py, &events)?.unbind())
    }

    /// Ranked lessons for a query dict (`{"context": {"repo": ...,
    /// "paths": [...]}, "text": ..., "embedding": [...], ...}`), as
    /// `{lesson, score, warnings}` dicts.
    fn recall(
        &self,
        py: Python<'_>,
        tenant_id: &str,
        query: Bound<'_, PyAny>,
    ) -> PyResult<Py<PyAny>> {
        let query: LessonQuery = depythonize(&query)?;
        let tenant_id = tenant_id.to_string();
        let engine = Arc::clone(&self.engine);
        let recalled = block(py, async move {
            let e = &engine;
            e.lessons(&tenant_id).recall(&query).await
        })?;
        Ok(pythonize(py, &recalled)?.unbind())
    }

    /// Recalls, renders the results as one delimited data block of at
    /// most `max_chars`, and records the included lessons as shown.
    /// Returns `{text, included, omitted}`; `text` is empty when nothing
    /// applies.
    #[pyo3(signature = (tenant_id, query, max_chars=z3rno_engine::lessons::DEFAULT_MAX_CHARS))]
    fn recall_rendered(
        &self,
        py: Python<'_>,
        tenant_id: &str,
        query: Bound<'_, PyAny>,
        max_chars: usize,
    ) -> PyResult<Py<PyAny>> {
        let query: LessonQuery = depythonize(&query)?;
        let tenant_id = tenant_id.to_string();
        let engine = Arc::clone(&self.engine);
        let rendered = block(py, async move {
            let e = &engine;
            let lessons = e.lessons(&tenant_id);
            let recalled = lessons.recall(&query).await?;
            let rendered = render_for_agent(&recalled, max_chars, chrono::Utc::now());
            lessons.mark_shown(&rendered.included).await?;
            Ok(rendered)
        })?;
        Ok(pythonize(py, &rendered)?.unbind())
    }
}

#[pymodule]
fn z3rno(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Client>()?;
    m.add_class::<Advanced>()?;
    m.add_class::<Lessons>()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_tier_accepts_every_snake_case_variant() {
        assert_eq!(parse_tier("working"), Ok(Tier::Working));
        assert_eq!(parse_tier("episodic"), Ok(Tier::Episodic));
        assert_eq!(parse_tier("semantic"), Ok(Tier::Semantic));
        assert_eq!(parse_tier("procedural"), Ok(Tier::Procedural));
    }

    #[test]
    fn parse_tier_rejects_unknown_value() {
        let err = parse_tier("bogus").unwrap_err();
        assert!(err.contains("bogus"));
    }

    #[test]
    fn tier_to_str_matches_backend_tier() {
        assert_eq!(tier_to_str(BackendTier::Embedded), "embedded");
        assert_eq!(tier_to_str(BackendTier::Postgres), "postgres");
    }

    #[test]
    fn parse_links_parses_valid_uuids() {
        let id = Uuid::new_v4();
        let links = parse_links(vec![(id.to_string(), "relates_to".to_string())]).unwrap();
        assert_eq!(links, vec![(id, "relates_to".to_string())]);
    }

    #[test]
    fn parse_links_rejects_invalid_uuid() {
        let err = parse_links(vec![("not-a-uuid".to_string(), "rel".to_string())]).unwrap_err();
        assert!(err.contains("not-a-uuid"));
    }
}
