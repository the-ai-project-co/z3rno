//! `/v1/memories*` — the store/recall/forget verb surface (engine::
//! MemoryEngine) plus the one public, unauthenticated `/v1/limits` probe.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;
use z3rno_engine::{ForgetProof, Memory, Tier};

use crate::auth::{AuthContext, Role};
use crate::error::{ApiError, ApiResult};
use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/memories", post(store))
        .route("/v1/memories/recall", post(recall))
        .route("/v1/memories/forget", post(forget))
        .route("/v1/limits", get(limits))
}

/// One graph edge from the memory being stored to an existing memory id —
/// the relationship name is caller-defined (episodic link, semantic
/// association, whatever the caller means by it).
#[derive(Debug, Deserialize)]
struct LinkRequest {
    target_id: Uuid,
    relationship: String,
}

#[derive(Debug, Deserialize)]
struct StoreRequest {
    tier: Tier,
    content: String,
    embedding: Option<Vec<f32>>,
    metadata: Option<serde_json::Value>,
    links: Option<Vec<LinkRequest>>,
}

async fn store(
    State(state): State<AppState>,
    ctx: AuthContext,
    Json(body): Json<StoreRequest>,
) -> ApiResult<(StatusCode, Json<Memory>)> {
    ctx.require_role(&[Role::Admin, Role::Write])?;

    // `metadata` defaults to `null` rather than `{}` when omitted — it's
    // stored as-is and returned as-is, so this preserves "caller didn't
    // send metadata" as a distinguishable value instead of silently
    // inventing an empty object.
    let metadata = body.metadata.unwrap_or(serde_json::Value::Null);
    let links = body
        .links
        .unwrap_or_default()
        .into_iter()
        .map(|l| (l.target_id, l.relationship))
        .collect();

    let memory = state
        .engine
        .store(
            &ctx.tenant_id,
            body.tier,
            body.content,
            body.embedding,
            metadata,
            links,
        )
        .await
        .map_err(anyhow::Error::from)?;

    Ok((StatusCode::CREATED, Json(memory)))
}

#[derive(Debug, Deserialize)]
struct RecallRequest {
    query: Vec<f32>,
    k: usize,
}

#[derive(Debug, Serialize)]
struct RecallResponse {
    results: Vec<Memory>,
    total: usize,
}

async fn recall(
    State(state): State<AppState>,
    ctx: AuthContext,
    Json(body): Json<RecallRequest>,
) -> ApiResult<Json<RecallResponse>> {
    ctx.require_role(&[Role::Admin, Role::Write, Role::Read])?;

    let results = state
        .engine
        .recall(&ctx.tenant_id, body.query, body.k)
        .await
        .map_err(anyhow::Error::from)?;
    let total = results.len();
    Ok(Json(RecallResponse { results, total }))
}

#[derive(Debug, Deserialize)]
struct ForgetRequest {
    id: Uuid,
}

async fn forget(
    State(state): State<AppState>,
    ctx: AuthContext,
    Json(body): Json<ForgetRequest>,
) -> ApiResult<Json<ForgetProof>> {
    ctx.require_role(&[Role::Admin, Role::Write])?;

    match state
        .engine
        .forget(&ctx.tenant_id, body.id)
        .await
        .map_err(anyhow::Error::from)?
    {
        Some(proof) => Ok(Json(proof)),
        None => Err(ApiError::NotFound(format!(
            "no memory {} to forget",
            body.id
        ))),
    }
}

/// Static placeholder caps — nothing in `z3rno-engine` enforces content
/// size, metadata size, or recall `k` today. These mirror the old system's
/// `/v1/limits` shape so clients built against it keep working, but they
/// are not real enforced limits yet; treat them as documentation of
/// intent, not a guarantee.
async fn limits() -> impl IntoResponse {
    Json(json!({
        "max_content_bytes": 1_048_576,
        "max_metadata_bytes": 65_536,
        "max_recall_k": 100
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::AuthConfig;
    use crate::cache::SqliteCacheBackend;
    use std::sync::Arc;
    use z3rno_engine::MemoryEngine;

    fn test_state() -> AppState {
        // `NamedTempFile::path()` on a temporary not bound to a variable
        // gets deleted the instant this statement ends, and rusqlite then
        // fails writes against the now-missing inode ("database file has
        // moved") — `keep()` disables that delete-on-drop so the file
        // outlives the connection for the test's duration.
        let db_path = tempfile::NamedTempFile::new()
            .unwrap()
            .into_temp_path()
            .keep()
            .unwrap();
        let engine = MemoryEngine::embedded(&db_path).unwrap();
        let cache = SqliteCacheBackend::open_in_memory().unwrap();
        AppState {
            engine: Arc::new(engine),
            cache: Arc::new(cache),
            auth: Arc::new(AuthConfig {
                jwt_secret: "test".into(),
                superadmin_api_key: None,
            }),
        }
    }

    fn ctx(role: Role) -> AuthContext {
        AuthContext {
            tenant_id: "test-tenant".to_string(),
            role,
        }
    }

    #[tokio::test]
    async fn store_then_recall_finds_the_memory() {
        let state = test_state();
        let resp = store(
            State(state.clone()),
            ctx(Role::Admin),
            Json(StoreRequest {
                tier: Tier::Working,
                content: "hello".to_string(),
                embedding: Some(vec![1.0, 0.0]),
                metadata: None,
                links: None,
            }),
        )
        .await
        .unwrap();
        assert_eq!(resp.0, StatusCode::CREATED);
        assert_eq!(resp.1 .0.content, "hello");

        let recalled = recall(
            State(state),
            ctx(Role::Read),
            Json(RecallRequest {
                query: vec![1.0, 0.0],
                k: 5,
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(recalled.total, 1);
        assert_eq!(recalled.results[0].content, "hello");
    }

    #[tokio::test]
    async fn forget_on_nonexistent_id_returns_not_found() {
        let state = test_state();
        let err = forget(
            State(state),
            ctx(Role::Admin),
            Json(ForgetRequest { id: Uuid::new_v4() }),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ApiError::NotFound(_)));
    }

    #[tokio::test]
    async fn forget_then_recall_no_longer_finds_it() {
        let state = test_state();
        let stored = state
            .engine
            .store(
                "test-tenant",
                Tier::Working,
                "bye".to_string(),
                Some(vec![1.0, 0.0]),
                serde_json::Value::Null,
                vec![],
            )
            .await
            .unwrap();

        let _ = forget(
            State(state.clone()),
            ctx(Role::Admin),
            Json(ForgetRequest { id: stored.id }),
        )
        .await
        .unwrap();

        let recalled = recall(
            State(state),
            ctx(Role::Read),
            Json(RecallRequest {
                query: vec![1.0, 0.0],
                k: 5,
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(recalled.total, 0);
    }

    #[tokio::test]
    async fn recall_rejects_wrong_role() {
        let state = test_state();
        let err = recall(
            State(state),
            ctx(Role::Audit),
            Json(RecallRequest {
                query: vec![1.0, 0.0],
                k: 5,
            }),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ApiError::Forbidden(_)));
    }

    #[tokio::test]
    async fn limits_returns_static_body() {
        let resp = limits().await;
        let _ = resp;
    }
}
