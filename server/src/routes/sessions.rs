//! `/v1/sessions*` — pure server-side session state, not a memory-engine
//! concept. Stored via `state.cache` (any `CacheBackend`) with a 24h TTL,
//! same shape the old system kept in Redis directly.

use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::AuthContext;
use crate::error::{ApiError, ApiResult};
use crate::AppState;

const SESSION_TTL: Duration = Duration::from_secs(24 * 60 * 60);

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/sessions", post(create))
        .route("/v1/sessions/{id}", get(fetch))
        .route("/v1/sessions/{id}/end", post(end))
}

/// What's actually persisted in the cache under `session:{id}` — just
/// enough to answer "whose session is this, and when did it start", since
/// everything else about a session (its id, its expiry) is derivable from
/// the cache key and the TTL rather than needing to be stored too.
#[derive(Debug, Serialize, Deserialize)]
struct SessionRecord {
    tenant_id: String,
    created_at: DateTime<Utc>,
}

fn cache_key(id: Uuid) -> String {
    format!("session:{id}")
}

#[derive(Debug, Serialize)]
struct CreateResponse {
    session_id: Uuid,
    expires_at: DateTime<Utc>,
}

async fn create(
    State(state): State<AppState>,
    ctx: AuthContext,
) -> ApiResult<Json<CreateResponse>> {
    let id = Uuid::new_v4();
    let now = Utc::now();
    let record = SessionRecord {
        tenant_id: ctx.tenant_id,
        created_at: now,
    };
    let value = serde_json::to_string(&record).map_err(anyhow::Error::from)?;
    state
        .cache
        .set(&cache_key(id), value, Some(SESSION_TTL))
        .await
        .map_err(|e| ApiError::Internal(e.into()))?;

    Ok(Json(CreateResponse {
        session_id: id,
        expires_at: now + chrono::Duration::seconds(SESSION_TTL.as_secs() as i64),
    }))
}

#[derive(Debug, Serialize)]
struct FetchResponse {
    session_id: Uuid,
    tenant_id: String,
    created_at: DateTime<Utc>,
}

async fn fetch(
    State(state): State<AppState>,
    _ctx: AuthContext,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<FetchResponse>> {
    let value = state
        .cache
        .get(&cache_key(id))
        .await
        .map_err(|e| ApiError::Internal(e.into()))?
        .ok_or_else(|| ApiError::NotFound(format!("no session {id}")))?;
    let record: SessionRecord = serde_json::from_str(&value).map_err(anyhow::Error::from)?;

    Ok(Json(FetchResponse {
        session_id: id,
        tenant_id: record.tenant_id,
        created_at: record.created_at,
    }))
}

async fn end(
    State(state): State<AppState>,
    _ctx: AuthContext,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    state
        .cache
        .delete(&cache_key(id))
        .await
        .map_err(|e| ApiError::Internal(e.into()))?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{AuthConfig, Role};
    use crate::cache::SqliteCacheBackend;
    use std::sync::Arc;
    use z3rno_engine::MemoryEngine;

    fn test_state() -> AppState {
        // See the identical comment in routes/memories.rs's test_state:
        // `keep()` avoids delete-on-drop of the temp file out from under
        // the still-open SQLite connection.
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

    fn ctx() -> AuthContext {
        AuthContext {
            tenant_id: "test-tenant".to_string(),
            role: Role::Read,
        }
    }

    #[tokio::test]
    async fn session_round_trips_through_create_get_end() {
        let state = test_state();

        let created = create(State(state.clone()), ctx()).await.unwrap().0;

        let fetched = fetch(State(state.clone()), ctx(), Path(created.session_id))
            .await
            .unwrap()
            .0;
        assert_eq!(fetched.session_id, created.session_id);
        assert_eq!(fetched.tenant_id, "test-tenant");

        let ended = end(State(state.clone()), ctx(), Path(created.session_id))
            .await
            .unwrap();
        assert_eq!(ended, StatusCode::NO_CONTENT);

        let err = fetch(State(state), ctx(), Path(created.session_id))
            .await
            .unwrap_err();
        assert!(matches!(err, ApiError::NotFound(_)));
    }

    #[tokio::test]
    async fn fetch_on_unknown_id_is_not_found() {
        let state = test_state();
        let err = fetch(State(state), ctx(), Path(Uuid::new_v4()))
            .await
            .unwrap_err();
        assert!(matches!(err, ApiError::NotFound(_)));
    }
}
