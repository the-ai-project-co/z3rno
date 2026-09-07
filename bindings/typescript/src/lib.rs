#![deny(clippy::all)]

//! The real napi-rs bindings over `z3rno-engine` (0006.1/0006.2/0006.4):
//! `Client.connect()` -> `store`/`recall`/`forget`, `client.advanced.audit`.
//! Async-only — every I/O method returns a native `Promise`, run on napi-rs's
//! managed Tokio runtime (`tokio_rt` feature). `tier()` is the one exception:
//! no I/O, so it's a plain sync getter.
//!
//! Kept deliberately thin: this module maps napi's JS-facing types onto
//! `z3rno_engine::MemoryEngine`'s calls. All the actual logic (tier/uuid
//! parsing, error mapping, DTO conversion) is plain Rust functions with no
//! napi types in their signatures, so it's unit-tested below with ordinary
//! `#[test]`s — no JS runtime required to run `cargo test`.

use std::sync::Arc;

use napi::bindgen_prelude::*;
use napi_derive::napi;
use serde_json::Value as JsonValue;
use uuid::Uuid;

use z3rno_engine::backend::BackendError;
use z3rno_engine::{
    AuditEvent, AuditOperation, BackendTier, ForgetProof, Memory, MemoryEngine, Tier,
};

const DEFAULT_EMBEDDED_PATH: &str = "z3rno.db";

// ---------------------------------------------------------------------
// Plain Rust helpers — no napi types, unit-tested at the bottom of this
// file with no live Node/JS runtime needed.
// ---------------------------------------------------------------------

fn parse_tier(s: &str) -> std::result::Result<Tier, String> {
    match s {
        "working" => Ok(Tier::Working),
        "episodic" => Ok(Tier::Episodic),
        "semantic" => Ok(Tier::Semantic),
        "procedural" => Ok(Tier::Procedural),
        other => Err(format!(
            "invalid tier {other:?}: expected one of \"working\", \"episodic\", \"semantic\", \"procedural\""
        )),
    }
}

fn tier_to_js_str(tier: Tier) -> &'static str {
    match tier {
        Tier::Working => "working",
        Tier::Episodic => "episodic",
        Tier::Semantic => "semantic",
        Tier::Procedural => "procedural",
    }
}

fn backend_tier_to_js_str(tier: BackendTier) -> &'static str {
    match tier {
        BackendTier::Embedded => "embedded",
        BackendTier::Postgres => "postgres",
    }
}

fn audit_operation_to_js_str(op: AuditOperation) -> &'static str {
    match op {
        AuditOperation::Store => "store",
        AuditOperation::Forget => "forget",
    }
}

fn parse_uuid(field: &str, s: &str) -> std::result::Result<Uuid, String> {
    Uuid::parse_str(s).map_err(|e| format!("invalid {field} {s:?}: {e}"))
}

/// napi-rs only implements `FromNapiValue` for `f64` (JS's one number
/// type), not `f32` — so embedding/query vectors cross the FFI boundary as
/// `Vec<f64>` and get narrowed here before reaching the engine, which
/// stores vectors as `Vec<f32>`.
fn to_f32_vec(v: Vec<f64>) -> Vec<f32> {
    v.into_iter().map(|x| x as f32).collect()
}

/// Maps a `BackendError` to a `napi::Error`, preserving the original message.
/// Both variants become `Status::GenericFailure` — neither maps cleanly onto
/// one of napi's more specific statuses (there's no "not found" status, and
/// `GenericFailure` is what napi-rs itself recommends for domain errors);
/// the distinction survives in the message text instead.
fn map_backend_error(err: BackendError) -> Error {
    match err {
        BackendError::NotFound(msg) => {
            Error::new(Status::GenericFailure, format!("not found: {msg}"))
        }
        BackendError::Storage(e) => {
            Error::new(Status::GenericFailure, format!("storage error: {e}"))
        }
    }
}

fn invalid_arg(message: impl Into<String>) -> Error {
    Error::new(Status::InvalidArg, message.into())
}

fn memory_to_dto(m: Memory) -> MemoryDto {
    MemoryDto {
        id: m.id.to_string(),
        tenant_id: m.tenant_id,
        tier: tier_to_js_str(m.tier).to_string(),
        content: m.content,
        metadata: m.metadata,
        created_at: m.created_at.to_rfc3339(),
    }
}

fn forget_proof_to_dto(p: ForgetProof) -> ForgetProofDto {
    ForgetProofDto {
        audit_event_id: p.audit_event_id.to_string(),
        hash: p.hash,
    }
}

fn audit_event_to_dto(e: AuditEvent) -> AuditEventDto {
    AuditEventDto {
        id: e.id.to_string(),
        tenant_id: e.tenant_id,
        operation: audit_operation_to_js_str(e.operation).to_string(),
        memory_id: e.memory_id.to_string(),
        at: e.at.to_rfc3339(),
        prev_hash: e.prev_hash,
        hash: e.hash,
    }
}

fn parse_links(links: Vec<LinkInput>) -> std::result::Result<Vec<(Uuid, String)>, String> {
    links
        .into_iter()
        .map(|l| parse_uuid("targetId", &l.target_id).map(|id| (id, l.relationship)))
        .collect()
}

// ---------------------------------------------------------------------
// JS-facing DTOs.
// ---------------------------------------------------------------------

/// `client.store(...)` input. `embedding` and `links` are optional (a
/// memory with no embedding simply isn't recallable by similarity search;
/// no links means no graph edges).
#[napi(object)]
pub struct StoreOptions {
    pub tenant_id: String,
    /// One of "working" | "episodic" | "semantic" | "procedural".
    pub tier: String,
    pub content: String,
    pub embedding: Option<Vec<f64>>,
    pub metadata: JsonValue,
    pub links: Option<Vec<LinkInput>>,
}

/// One graph edge from the memory being stored to an existing memory.
#[napi(object)]
pub struct LinkInput {
    pub target_id: String,
    pub relationship: String,
}

/// `client.recall(...)` input.
#[napi(object)]
pub struct RecallOptions {
    pub tenant_id: String,
    pub query: Vec<f64>,
    pub k: u32,
}

/// `client.forget(...)` input.
#[napi(object)]
pub struct ForgetOptions {
    pub tenant_id: String,
    pub id: String,
}

/// `Client.connect(...)` input. `backend` defaults to `"embedded"`; `path`
/// defaults to `"z3rno.db"` in the current working directory when the
/// embedded backend is used and `path` is omitted.
#[napi(object)]
pub struct ConnectOptions {
    /// One of "embedded" | "postgres". Defaults to "embedded".
    pub backend: Option<String>,
    pub path: Option<String>,
    pub connection_string: Option<String>,
}

#[napi(object)]
pub struct MemoryDto {
    pub id: String,
    pub tenant_id: String,
    pub tier: String,
    pub content: String,
    pub metadata: JsonValue,
    /// RFC 3339 timestamp.
    pub created_at: String,
}

#[napi(object)]
pub struct ForgetProofDto {
    pub audit_event_id: String,
    pub hash: String,
}

#[napi(object)]
pub struct AuditEventDto {
    pub id: String,
    pub tenant_id: String,
    /// "store" | "forget".
    pub operation: String,
    pub memory_id: String,
    /// RFC 3339 timestamp.
    pub at: String,
    pub prev_hash: Option<String>,
    pub hash: String,
}

// ---------------------------------------------------------------------
// Client.
// ---------------------------------------------------------------------

/// A connected z3rno client. Construct with `Client.connect(...)`, never
/// `new Client()` — connecting the embedded backend is sync but connecting
/// Postgres needs an `await`, so both go through one consistent async
/// factory rather than a sync constructor plus a separate async path.
#[napi]
pub struct Client {
    engine: Arc<MemoryEngine>,
}

#[napi]
impl Client {
    #[napi(factory)]
    pub async fn connect(options: Option<ConnectOptions>) -> Result<Client> {
        let options = options.unwrap_or(ConnectOptions {
            backend: None,
            path: None,
            connection_string: None,
        });
        let backend = options.backend.as_deref().unwrap_or("embedded");
        let engine = match backend {
            "embedded" => {
                let path = options
                    .path
                    .unwrap_or_else(|| DEFAULT_EMBEDDED_PATH.to_string());
                MemoryEngine::embedded(path).map_err(map_backend_error)?
            }
            "postgres" => {
                let connection_string = options.connection_string.ok_or_else(|| {
                    invalid_arg("connectionString is required when backend is \"postgres\"")
                })?;
                MemoryEngine::postgres(&connection_string)
                    .await
                    .map_err(map_backend_error)?
            }
            other => {
                return Err(invalid_arg(format!(
                    "invalid backend {other:?}: expected \"embedded\" or \"postgres\""
                )))
            }
        };
        Ok(Client {
            engine: Arc::new(engine),
        })
    }

    /// Which tenant-isolation guarantee this client's backend provides:
    /// "embedded" or "postgres". No I/O, so this is sync (not a Promise).
    #[napi]
    pub fn tier(&self) -> String {
        backend_tier_to_js_str(self.engine.tier()).to_string()
    }

    #[napi]
    pub async fn store(&self, options: StoreOptions) -> Result<MemoryDto> {
        let tier = parse_tier(&options.tier).map_err(invalid_arg)?;
        let links = parse_links(options.links.unwrap_or_default()).map_err(invalid_arg)?;
        let memory = self
            .engine
            .store(
                &options.tenant_id,
                tier,
                options.content,
                options.embedding.map(to_f32_vec),
                options.metadata,
                links,
            )
            .await
            .map_err(map_backend_error)?;
        Ok(memory_to_dto(memory))
    }

    #[napi]
    pub async fn recall(&self, options: RecallOptions) -> Result<Vec<MemoryDto>> {
        let memories = self
            .engine
            .recall(
                &options.tenant_id,
                to_f32_vec(options.query),
                options.k as usize,
            )
            .await
            .map_err(map_backend_error)?;
        Ok(memories.into_iter().map(memory_to_dto).collect())
    }

    #[napi]
    pub async fn forget(&self, options: ForgetOptions) -> Result<Option<ForgetProofDto>> {
        let id = parse_uuid("id", &options.id).map_err(invalid_arg)?;
        let proof = self
            .engine
            .forget(&options.tenant_id, id)
            .await
            .map_err(map_backend_error)?;
        Ok(proof.map(forget_proof_to_dto))
    }

    /// Advanced operations namespace — currently just `.audit(tenantId)`,
    /// the one advanced operation the engine has. Mirrors the engine's own
    /// `MemoryEngine::advanced()` two-tier verb surface.
    #[napi(getter)]
    pub fn advanced(&self) -> Advanced {
        Advanced {
            engine: self.engine.clone(),
        }
    }
}

#[napi]
pub struct Advanced {
    engine: Arc<MemoryEngine>,
}

#[napi]
impl Advanced {
    /// The tenant's full audit chain, oldest first.
    #[napi]
    pub async fn audit(&self, tenant_id: String) -> Result<Vec<AuditEventDto>> {
        let events = self
            .engine
            .advanced()
            .audit(&tenant_id)
            .await
            .map_err(map_backend_error)?;
        Ok(events.into_iter().map(audit_event_to_dto).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_tier_accepts_all_four_snake_case_variants() {
        assert_eq!(parse_tier("working"), Ok(Tier::Working));
        assert_eq!(parse_tier("episodic"), Ok(Tier::Episodic));
        assert_eq!(parse_tier("semantic"), Ok(Tier::Semantic));
        assert_eq!(parse_tier("procedural"), Ok(Tier::Procedural));
    }

    #[test]
    fn parse_tier_rejects_unknown_value_with_descriptive_message() {
        let err = parse_tier("bogus").unwrap_err();
        assert!(
            err.contains("bogus"),
            "message should name the bad value: {err}"
        );
        assert!(
            err.contains("working"),
            "message should list valid values: {err}"
        );
    }

    #[test]
    fn tier_round_trips_through_js_str() {
        for tier in [
            Tier::Working,
            Tier::Episodic,
            Tier::Semantic,
            Tier::Procedural,
        ] {
            let s = tier_to_js_str(tier);
            assert_eq!(parse_tier(s), Ok(tier));
        }
    }

    #[test]
    fn backend_tier_to_js_str_matches_expected_strings() {
        assert_eq!(backend_tier_to_js_str(BackendTier::Embedded), "embedded");
        assert_eq!(backend_tier_to_js_str(BackendTier::Postgres), "postgres");
    }

    #[test]
    fn audit_operation_to_js_str_matches_expected_strings() {
        assert_eq!(audit_operation_to_js_str(AuditOperation::Store), "store");
        assert_eq!(audit_operation_to_js_str(AuditOperation::Forget), "forget");
    }

    #[test]
    fn to_f32_vec_narrows_each_element() {
        assert_eq!(
            to_f32_vec(vec![0.1, 0.2, 1.0]),
            vec![0.1f32, 0.2f32, 1.0f32]
        );
    }

    #[test]
    fn parse_uuid_accepts_valid_uuid() {
        let id = Uuid::new_v4();
        assert_eq!(parse_uuid("id", &id.to_string()), Ok(id));
    }

    #[test]
    fn parse_uuid_rejects_garbage_with_field_name_in_message() {
        let err = parse_uuid("targetId", "not-a-uuid").unwrap_err();
        assert!(
            err.contains("targetId"),
            "message should name the field: {err}"
        );
    }

    #[test]
    fn parse_links_parses_target_ids_and_preserves_relationship() {
        let id = Uuid::new_v4();
        let links = vec![LinkInput {
            target_id: id.to_string(),
            relationship: "caused_by".to_string(),
        }];
        let parsed = parse_links(links).unwrap();
        assert_eq!(parsed, vec![(id, "caused_by".to_string())]);
    }

    #[test]
    fn parse_links_surfaces_which_target_id_is_invalid() {
        let links = vec![LinkInput {
            target_id: "nope".to_string(),
            relationship: "r".to_string(),
        }];
        let err = parse_links(links).unwrap_err();
        assert!(err.contains("nope"));
    }

    #[test]
    fn map_backend_error_preserves_not_found_message() {
        let err = map_backend_error(BackendError::NotFound("memory abc123".to_string()));
        assert_eq!(err.status, Status::GenericFailure);
        assert!(err.reason.contains("memory abc123"));
    }

    #[test]
    fn map_backend_error_preserves_storage_error_message() {
        let err = map_backend_error(BackendError::Storage(anyhow::anyhow!("disk full")));
        assert_eq!(err.status, Status::GenericFailure);
        assert!(err.reason.contains("disk full"));
    }

    #[test]
    fn invalid_arg_uses_invalid_arg_status() {
        let err = invalid_arg("bad input");
        assert_eq!(err.status, Status::InvalidArg);
        assert_eq!(err.reason, "bad input");
    }

    #[test]
    fn memory_to_dto_maps_every_field() {
        let id = Uuid::new_v4();
        let memory = Memory {
            id,
            tenant_id: "t1".to_string(),
            tier: Tier::Semantic,
            content: "hello".to_string(),
            metadata: serde_json::json!({"k": "v"}),
            created_at: chrono::Utc::now(),
        };
        let dto = memory_to_dto(memory.clone());
        assert_eq!(dto.id, id.to_string());
        assert_eq!(dto.tenant_id, "t1");
        assert_eq!(dto.tier, "semantic");
        assert_eq!(dto.content, "hello");
        assert_eq!(dto.metadata, serde_json::json!({"k": "v"}));
        assert_eq!(dto.created_at, memory.created_at.to_rfc3339());
    }

    #[test]
    fn forget_proof_to_dto_maps_every_field() {
        let event_id = Uuid::new_v4();
        let dto = forget_proof_to_dto(ForgetProof {
            audit_event_id: event_id,
            hash: "deadbeef".to_string(),
        });
        assert_eq!(dto.audit_event_id, event_id.to_string());
        assert_eq!(dto.hash, "deadbeef");
    }

    #[test]
    fn audit_event_to_dto_maps_every_field_including_none_prev_hash() {
        let event = AuditEvent::next("t1", AuditOperation::Store, Uuid::new_v4(), None);
        let dto = audit_event_to_dto(event.clone());
        assert_eq!(dto.id, event.id.to_string());
        assert_eq!(dto.tenant_id, "t1");
        assert_eq!(dto.operation, "store");
        assert_eq!(dto.memory_id, event.memory_id.to_string());
        assert_eq!(dto.at, event.at.to_rfc3339());
        assert_eq!(dto.prev_hash, None);
        assert_eq!(dto.hash, event.hash);
    }
}
