//! `/v1/audit` — the tenant's append-only, hash-chained audit chain
//! (`engine::MemoryEngine::advanced().audit`).

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use z3rno_engine::AuditEvent;

use crate::auth::{AuthContext, Role};
use crate::error::ApiResult;
use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/v1/audit", get(audit))
}

#[derive(Debug, Serialize)]
struct AuditResponse {
    events: Vec<AuditEvent>,
}

/// The old Python API paginated this at the SQL layer; `z3rno-engine`'s
/// `advanced().audit()` has no pagination parameter yet, so this returns
/// the tenant's full chain. A known simplification to revisit if a real
/// tenant's audit log grows large — not a bug to fake pagination around
/// here.
async fn audit(State(state): State<AppState>, ctx: AuthContext) -> ApiResult<Json<AuditResponse>> {
    ctx.require_role(&[Role::Admin, Role::Write, Role::Read, Role::Audit])?;

    let events = state
        .engine
        .advanced()
        .audit(&ctx.tenant_id)
        .await
        .map_err(anyhow::Error::from)?;
    Ok(Json(AuditResponse { events }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::AuthConfig;
    use crate::cache::SqliteCacheBackend;
    use crate::error::ApiError;
    use std::sync::Arc;
    use z3rno_engine::{MemoryEngine, Tier};

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

    fn ctx(role: Role) -> AuthContext {
        AuthContext {
            tenant_id: "test-tenant".to_string(),
            role,
        }
    }

    #[tokio::test]
    async fn audit_log_grows_after_store_and_forget() {
        let state = test_state();

        let empty = audit(State(state.clone()), ctx(Role::Audit)).await.unwrap();
        assert_eq!(empty.0.events.len(), 0);

        let memory = state
            .engine
            .store(
                "test-tenant",
                Tier::Working,
                "content".to_string(),
                None,
                serde_json::Value::Null,
                vec![],
            )
            .await
            .unwrap();

        let after_store = audit(State(state.clone()), ctx(Role::Audit)).await.unwrap();
        assert_eq!(after_store.0.events.len(), 1);

        state.engine.forget("test-tenant", memory.id).await.unwrap();

        let after_forget = audit(State(state), ctx(Role::Audit)).await.unwrap();
        assert_eq!(after_forget.0.events.len(), 2);
    }

    #[tokio::test]
    async fn audit_rejects_wrong_role() {
        // Audit has no role left out of its allow-list except none — use a
        // role guaranteed absent from it by constructing the call directly
        // and checking `require_role`'s real effect through the handler.
        // All four non-superadmin roles are allowed for /v1/audit, so this
        // exercises the one that isn't: an unauthenticated caller never
        // reaches the handler at all (enforced by the `AuthContext`
        // extractor, not this file) — instead we assert Superadmin, which
        // the route contract does not include, is rejected.
        let state = test_state();
        let err = audit(State(state), ctx(Role::Superadmin))
            .await
            .unwrap_err();
        assert!(matches!(err, ApiError::Forbidden(_)));
    }
}
