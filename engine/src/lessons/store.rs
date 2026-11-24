//! `MemoryEngine::lessons()`: the lesson API over the engine's existing
//! storage. A lesson is a procedural-tier memory whose metadata carries the
//! validated record; its content is the searchable text (what gets
//! embedded). Status history goes to `lesson_event` records.

use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::backend::{BackendError, BackendResult, ErrorCode, Record};
use crate::engine::{MemoryEngine, KIND_MEMORY, TYPED_KIND_KEY};
use crate::forget::derived_record_id;
use crate::model::{Memory, Tier};
use crate::ForgetProof;

use super::lifecycle::{
    authorize, require_human, LessonAction, LessonEvent, Transition, KIND_LESSON_EVENT,
};
use super::model::{
    invalid, Actor, Lesson, LessonDraft, LessonStatus, MAX_SERIALIZED_BYTES, SCHEMA_VERSION,
};
use super::recall::{rank, Candidate, LessonQuery, RecalledLesson};
use super::redact::Redactor;
use super::scope::applicability;

/// Metadata marker that distinguishes a lesson from an ordinary memory.
const METADATA_KIND: &str = "lesson";
/// A lesson older than this (since confirmation) is shown with a warning.
const STALE_AFTER_DAYS: i64 = 180;
/// Vector candidates fetched per applicable lesson before post-filtering.
const VECTOR_OVERFETCH: usize = 4;
const VECTOR_FETCH_CAP: usize = 1_000;

/// Shown counters live in their own record so `mark_shown` never rewrites
/// the lesson (which could undo a concurrent human edit or transition).
const KIND_LESSON_SHOWN: &str = "lesson_shown";

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct Shown {
    lesson_id: Uuid,
    count: u64,
    last_at: DateTime<Utc>,
}

impl Shown {
    fn record_id(lesson_id: Uuid) -> Uuid {
        derived_record_id(KIND_LESSON_SHOWN, lesson_id)
    }

    fn apply(self, lesson: &mut Lesson) {
        lesson.shown_count = self.count;
        lesson.last_shown_at = Some(self.last_at);
    }
}

/// Graph relationship names lessons use for their links.
pub const REL_DERIVED_FROM: &str = "derived_from";
pub const REL_SUPERSEDES: &str = "supersedes";

/// Which lessons `Lessons::list` returns.
#[derive(Debug, Clone, Default)]
pub struct LessonFilter {
    pub status: Option<LessonStatus>,
    pub repo: Option<String>,
}

/// Borrowed handle returned by `MemoryEngine::lessons()`.
pub struct Lessons<'a> {
    engine: &'a MemoryEngine,
    tenant_id: &'a str,
    redactor: Redactor,
}

impl MemoryEngine {
    /// The lesson API for one tenant.
    pub fn lessons<'a>(&'a self, tenant_id: &'a str) -> Lessons<'a> {
        Lessons {
            engine: self,
            tenant_id,
            redactor: Redactor::default(),
        }
    }
}

impl<'a> Lessons<'a> {
    /// Overrides the default redaction settings (e.g. to also redact
    /// email addresses).
    pub fn with_redactor(mut self, redactor: Redactor) -> Self {
        self.redactor = redactor;
        self
    }

    /// Redacts and validates `draft` without storing it, returning exactly
    /// what `create` would store. Interfaces use this to preview a lesson.
    pub fn prepare(&self, mut draft: LessonDraft) -> BackendResult<LessonDraft> {
        self.redactor.redact_draft(&mut draft);
        draft.validate()?;
        Ok(draft)
    }

    /// Stores a new lesson. Agents and CI can only create `Candidate`s; a
    /// human may create one already `Confirmed`. `derived_from` ids must be
    /// existing memories and become `derived_from` graph edges.
    pub async fn create(
        &self,
        actor: &Actor,
        draft: LessonDraft,
        initial: LessonStatus,
        embedding: Option<Vec<f32>>,
    ) -> BackendResult<Lesson> {
        actor.validate()?;
        match initial {
            LessonStatus::Candidate => {}
            LessonStatus::Confirmed => require_human(actor, "confirm")?,
            other => {
                return Err(invalid(format!(
                    "a new lesson can't start as {}",
                    other.as_str()
                )))
            }
        }
        let draft = self.prepare(draft)?;
        self.require_live(&draft.derived_from).await?;

        let now = Utc::now();
        let confirmed = initial == LessonStatus::Confirmed;
        let lesson = Lesson {
            id: Uuid::new_v4(),
            schema_version: SCHEMA_VERSION,
            draft,
            status: initial,
            origin: actor.clone(),
            created_at: now,
            confirmed_by: confirmed.then(|| actor.clone()),
            confirmed_at: confirmed.then_some(now),
            last_shown_at: None,
            shown_count: 0,
            superseded_by: None,
        };
        let links = lesson
            .draft
            .derived_from
            .iter()
            .map(|id| (*id, REL_DERIVED_FROM.to_string()))
            .collect();
        self.engine
            .store_memory(to_memory(self.tenant_id, &lesson)?, embedding, links)
            .await?;
        self.record_event(
            lesson.id,
            LessonAction::Created { status: initial },
            actor,
            None,
        )
        .await?;
        Ok(lesson)
    }

    pub async fn get(&self, id: Uuid) -> BackendResult<Option<Lesson>> {
        let record = self.engine.engine_backend.get(self.tenant_id, id).await?;
        let mut lesson = match record {
            Some(r) if r.kind == KIND_MEMORY => {
                match lesson_from_memory(&MemoryEngine::record_to_memory(r)?)? {
                    Some(lesson) => lesson,
                    None => return Ok(None),
                }
            }
            _ => return Ok(None),
        };
        if let Some(shown) = self.shown(id).await? {
            shown.apply(&mut lesson);
        }
        Ok(Some(lesson))
    }

    async fn shown(&self, lesson_id: Uuid) -> BackendResult<Option<Shown>> {
        match self
            .engine
            .engine_backend
            .get(self.tenant_id, Shown::record_id(lesson_id))
            .await?
        {
            Some(r) if r.kind == KIND_LESSON_SHOWN => Ok(Some(
                serde_json::from_value(r.data).map_err(anyhow::Error::from)?,
            )),
            _ => Ok(None),
        }
    }

    /// Like `get`, but a missing id is an error.
    async fn require(&self, id: Uuid) -> BackendResult<Lesson> {
        self.get(id)
            .await?
            .ok_or_else(|| BackendError::NotFound(format!("lesson {id}")))
    }

    /// Every lesson matching `filter`, newest first.
    pub async fn list(&self, filter: &LessonFilter) -> BackendResult<Vec<Lesson>> {
        let mut lessons: Vec<Lesson> = self
            .all()
            .await?
            .into_iter()
            .filter(|l| filter.status.is_none_or(|s| l.status == s))
            .filter(|l| {
                filter
                    .repo
                    .as_deref()
                    .is_none_or(|repo| l.draft.scope.repo.as_deref() == Some(repo))
            })
            .collect();
        lessons.sort_by(|a, b| b.created_at.cmp(&a.created_at).then(a.id.cmp(&b.id)));
        Ok(lessons)
    }

    /// shortcut: a full scan of the tenant's memories; fine for a local
    /// developer's store, upgrade to an indexed scope table if tenants
    /// grow to many thousands of memories.
    async fn all(&self) -> BackendResult<Vec<Lesson>> {
        let records = self
            .engine
            .engine_backend
            .list(self.tenant_id, KIND_MEMORY)
            .await?;
        let shown: HashMap<Uuid, Shown> = self
            .engine
            .engine_backend
            .list(self.tenant_id, KIND_LESSON_SHOWN)
            .await?
            .into_iter()
            .map(|r| serde_json::from_value::<Shown>(r.data).map(|s| (s.lesson_id, s)))
            .collect::<Result<_, _>>()
            .map_err(anyhow::Error::from)?;
        let mut lessons = Vec::new();
        for record in records {
            if let Some(mut lesson) = lesson_from_memory(&MemoryEngine::record_to_memory(record)?)?
            {
                if let Some(s) = shown.get(&lesson.id) {
                    s.apply(&mut lesson);
                }
                lessons.push(lesson);
            }
        }
        Ok(lessons)
    }

    /// Replaces a lesson's authored content (human only). Status, origin,
    /// counters and `derived_from` links are kept. Pass the new text's
    /// embedding; `None` removes the old one rather than leaving a vector
    /// that describes text which no longer exists.
    pub async fn edit(
        &self,
        actor: &Actor,
        id: Uuid,
        draft: LessonDraft,
        embedding: Option<Vec<f32>>,
        note: Option<String>,
    ) -> BackendResult<Lesson> {
        require_human(actor, "edit")?;
        let mut lesson = self.require(id).await?;
        let mut draft = self.prepare(draft)?;
        if !draft.derived_from.is_empty() && draft.derived_from != lesson.draft.derived_from {
            return Err(invalid("derived_from can't change after creation"));
        }
        draft.derived_from = std::mem::take(&mut lesson.draft.derived_from);
        lesson.draft = draft;
        self.save(&lesson).await?;
        match embedding {
            Some(e) => {
                self.engine
                    .vector_backend
                    .upsert(self.tenant_id, id, e)
                    .await?
            }
            None => {
                self.engine
                    .vector_backend
                    .delete(self.tenant_id, id)
                    .await?;
            }
        }
        self.record_event(id, LessonAction::Edited, actor, note)
            .await?;
        Ok(lesson)
    }

    /// Applies a status transition (human only; see `lifecycle`).
    pub async fn transition(
        &self,
        actor: &Actor,
        id: Uuid,
        transition: Transition,
        note: Option<String>,
    ) -> BackendResult<Lesson> {
        let mut lesson = self.require(id).await?;
        let from = lesson.status;
        let to = authorize(transition, from, actor)?;
        lesson.status = to;
        if to == LessonStatus::Confirmed {
            lesson.confirmed_by = Some(actor.clone());
            lesson.confirmed_at = Some(Utc::now());
        }
        self.save(&lesson).await?;
        self.record_event(
            id,
            LessonAction::StatusChanged {
                from,
                to,
                superseded_by: None,
            },
            actor,
            note,
        )
        .await?;
        Ok(lesson)
    }

    /// Retires `old_id` in favour of `new_id` (human only), recording the
    /// replacement and a `supersedes` edge from the new lesson to the old.
    pub async fn supersede(
        &self,
        actor: &Actor,
        old_id: Uuid,
        new_id: Uuid,
        note: Option<String>,
    ) -> BackendResult<Lesson> {
        if old_id == new_id {
            return Err(invalid("a lesson can't supersede itself"));
        }
        let replacement = self.require(new_id).await?;
        if !matches!(
            replacement.status,
            LessonStatus::Candidate | LessonStatus::Confirmed
        ) {
            return Err(BackendError::rejected(
                ErrorCode::InvalidTransition,
                format!(
                    "replacement lesson {new_id} is {}",
                    replacement.status.as_str()
                ),
            ));
        }
        let mut old = self.require(old_id).await?;
        let from = old.status;
        let to = authorize(Transition::Retire, from, actor)?;
        old.status = to;
        old.superseded_by = Some(new_id);
        self.save(&old).await?;
        self.engine
            .graph_backend
            .add_edge(
                self.tenant_id,
                crate::backend::GraphEdge {
                    source: new_id,
                    target: old_id,
                    relationship: REL_SUPERSEDES.to_string(),
                },
            )
            .await?;
        self.record_event(
            old_id,
            LessonAction::StatusChanged {
                from,
                to,
                superseded_by: Some(new_id),
            },
            actor,
            note,
        )
        .await?;
        Ok(old)
    }

    /// Deletes a lesson from every store through the engine's resumable
    /// forget protocol (human only). A pending forget finished elsewhere
    /// (generic `forget`, `complete_pending_forgets`) skips this bookkeeping.
    /// The history keeps who deleted which id, never
    /// the content. `Ok(None)` if there was no such lesson.
    pub async fn delete(&self, actor: &Actor, id: Uuid) -> BackendResult<Option<ForgetProof>> {
        require_human(actor, "delete")?;
        if self.get(id).await?.is_none() {
            // Not a lesson (or already gone): still let a pending forget
            // of this id finish, but never delete a non-lesson memory.
            if self
                .engine
                .advanced()
                .pending_forgets(self.tenant_id)
                .await?
                .iter()
                .all(|p| p.memory_id != id)
            {
                return Ok(None);
            }
        }
        let proof = self.engine.forget_any(self.tenant_id, id).await?;
        if proof.is_some() {
            self.engine
                .engine_backend
                .delete(self.tenant_id, Shown::record_id(id))
                .await?;
            self.record_event(id, LessonAction::Deleted, actor, None)
                .await?;
        }
        Ok(proof)
    }

    /// Deletes every lesson scoped to `repo`, returning the deleted ids.
    /// Stops at the first failure (a later call resumes it).
    pub async fn delete_by_repo(&self, actor: &Actor, repo: &str) -> BackendResult<Vec<Uuid>> {
        require_human(actor, "delete")?;
        // An earlier interrupted delete already hid its lesson from `list`,
        // so finish those first (through `delete`, for the bookkeeping). Their
        // repo is no longer known, so they aren't in the returned ids.
        for pending in self
            .engine
            .advanced()
            .pending_forgets(self.tenant_id)
            .await?
        {
            self.delete(actor, pending.memory_id).await?;
        }
        let filter = LessonFilter {
            status: None,
            repo: Some(repo.to_string()),
        };
        let mut deleted = Vec::new();
        for lesson in self.list(&filter).await? {
            if self.delete(actor, lesson.id).await?.is_some() {
                deleted.push(lesson.id);
            }
        }
        Ok(deleted)
    }

    /// Scoped recall: the lessons that apply to `query.context`, gated by
    /// status, ranked (see `recall`), capped at `query.effective_limit()`.
    /// With no repo in the context, nothing is returned unless the caller
    /// sets `include_global` (and then only global lessons match).
    pub async fn recall(&self, query: &LessonQuery) -> BackendResult<Vec<RecalledLesson>> {
        query.context.validate()?;
        if query.context.repo.is_none() && !query.include_global {
            return Ok(Vec::new());
        }
        let now = Utc::now();
        let candidates: Vec<Candidate> = self
            .all()
            .await?
            .into_iter()
            .filter(|l| match l.status {
                LessonStatus::Confirmed => true,
                LessonStatus::Candidate => query.include_candidates,
                LessonStatus::Retired | LessonStatus::Rejected => false,
            })
            .filter(|l| query.include_global || !l.draft.scope.global)
            .filter_map(|lesson| {
                let a = applicability(&lesson.draft.scope, &query.context)?;
                let warnings = warnings_for(&lesson, query, now);
                Some(Candidate {
                    specificity: a.specificity(),
                    lesson,
                    warnings,
                })
            })
            .collect();
        if candidates.is_empty() {
            return Ok(Vec::new());
        }

        let vector_order = match &query.embedding {
            Some(embedding) => {
                let k = (candidates.len() * VECTOR_OVERFETCH)
                    .clamp(query.effective_limit(), VECTOR_FETCH_CAP);
                self.engine
                    .vector_backend
                    .search(self.tenant_id, embedding.clone(), k)
                    .await?
                    .into_iter()
                    .map(|m| m.id)
                    .collect()
            }
            None => Vec::new(),
        };
        Ok(rank(
            candidates,
            query.text.as_deref(),
            &vector_order,
            query.effective_limit(),
        ))
    }

    /// Records that these lessons were shown to an agent (bumps
    /// `shown_count`, sets `last_shown_at`). Unknown ids are skipped.
    ///
    /// shortcut: read-modify-write, so two concurrent calls can lose an
    /// increment; the counter is advisory. Use an atomic backend op if it
    /// ever drives decisions.
    pub async fn mark_shown(&self, ids: &[Uuid]) -> BackendResult<()> {
        let now = Utc::now();
        for &id in ids {
            let Some(lesson) = self.get(id).await? else {
                continue;
            };
            let shown = Shown {
                lesson_id: id,
                count: lesson.shown_count + 1,
                last_at: now,
            };
            self.engine
                .engine_backend
                .put(Record {
                    id: Shown::record_id(id),
                    tenant_id: self.tenant_id.to_string(),
                    kind: KIND_LESSON_SHOWN.to_string(),
                    data: serde_json::to_value(shown).map_err(anyhow::Error::from)?,
                    created_at: now,
                })
                .await?;
        }
        Ok(())
    }

    /// A lesson's history, oldest first.
    pub async fn history(&self, id: Uuid) -> BackendResult<Vec<LessonEvent>> {
        let mut events = self
            .engine
            .engine_backend
            .list(self.tenant_id, KIND_LESSON_EVENT)
            .await?
            .into_iter()
            .map(|r| serde_json::from_value::<LessonEvent>(r.data).map_err(anyhow::Error::from))
            .collect::<Result<Vec<_>, _>>()?;
        events.retain(|e| e.lesson_id == id);
        events.sort_by_key(|e| e.at);
        Ok(events)
    }

    async fn save(&self, lesson: &Lesson) -> BackendResult<()> {
        let mut memory = to_memory(self.tenant_id, lesson)?;
        // Keep the original row's timestamp; only content/metadata change.
        memory.created_at = lesson.created_at;
        self.engine.put_memory(&memory).await
    }

    async fn record_event(
        &self,
        lesson_id: Uuid,
        action: LessonAction,
        actor: &Actor,
        note: Option<String>,
    ) -> BackendResult<()> {
        let note = note.map(|n| self.redactor.redact(&n));
        let event = LessonEvent::new(lesson_id, action, actor, note);
        self.engine
            .engine_backend
            .put(Record {
                id: event.id,
                tenant_id: self.tenant_id.to_string(),
                kind: KIND_LESSON_EVENT.to_string(),
                data: serde_json::to_value(&event).map_err(anyhow::Error::from)?,
                created_at: event.at,
            })
            .await
    }

    async fn require_live(&self, ids: &[Uuid]) -> BackendResult<()> {
        let live = self
            .engine
            .live_memory_ids(self.tenant_id, ids.to_vec())
            .await?;
        if let Some(missing) = ids.iter().find(|id| !live.contains(id)) {
            return Err(invalid(format!(
                "derived_from id {missing} is not a stored memory"
            )));
        }
        Ok(())
    }
}

fn warnings_for(lesson: &Lesson, query: &LessonQuery, now: chrono::DateTime<Utc>) -> Vec<String> {
    let mut warnings = Vec::new();
    let since = lesson.confirmed_at.unwrap_or(lesson.created_at);
    if now - since > Duration::days(STALE_AFTER_DAYS) {
        warnings.push(format!(
            "last confirmed {} days ago; check it still applies",
            (now - since).num_days()
        ));
    }
    for dep in &lesson.draft.scope.dependencies {
        if let Some(range) = &dep.range {
            if !query.context.dependencies.contains_key(&dep.name) {
                warnings.push(format!(
                    "applies to {} {range}; installed version unknown",
                    dep.name
                ));
            }
        }
    }
    warnings
}

fn to_memory(tenant_id: &str, lesson: &Lesson) -> BackendResult<Memory> {
    let metadata = json!({ TYPED_KIND_KEY: METADATA_KIND, "lesson": lesson });
    let size = serde_json::to_vec(&metadata)
        .map_err(anyhow::Error::from)?
        .len();
    if size > MAX_SERIALIZED_BYTES {
        return Err(invalid(format!(
            "lesson is {size} bytes serialized; at most {MAX_SERIALIZED_BYTES} allowed"
        )));
    }
    Ok(Memory {
        id: lesson.id,
        tenant_id: tenant_id.to_string(),
        tier: Tier::Procedural,
        content: lesson.searchable_text(),
        metadata,
        created_at: lesson.created_at,
    })
}

/// `Some` if `memory` carries a lesson record.
pub fn lesson_from_memory(memory: &Memory) -> BackendResult<Option<Lesson>> {
    // Only `to_memory` writes this shape: generic `store` refuses the key.
    // Tier and id are checked too, as a second line against a forged row.
    if memory.metadata.get(TYPED_KIND_KEY).and_then(|v| v.as_str()) != Some(METADATA_KIND)
        || memory.tier != Tier::Procedural
    {
        return Ok(None);
    }
    let value = memory
        .metadata
        .get("lesson")
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("lesson memory {} has no lesson payload", memory.id))?;
    let lesson: Lesson = serde_json::from_value(value).map_err(anyhow::Error::from)?;
    if lesson.id != memory.id {
        return Ok(None);
    }
    Ok(Some(lesson))
}
