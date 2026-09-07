//! Sub-slice 0005.3 — cross-tenant budget admin, ported in behavior from
//! the old system's `/v1/tenants/{org_id}/budgets` (superadmin-only sister
//! surface to a per-tenant `/v1/tenants/me/budgets`, which this slice
//! doesn't build — no caller needs it yet, and it's a straightforward
//! follow-up once one does). Flagged in plan-doc 0005 as the one feature
//! the old `z3rno-mcp` binding lagged behind on; built into the server
//! from day one here instead.
//!
//! The old system stored overrides in a `tenants.usage_budget` JSONB
//! column and used `SET LOCAL app.current_org_id` to reuse its RLS-scoped
//! SQL cross-tenant. This crate has no `tenants` table (the engine has no
//! such concept), so overrides live in `state.cache` under
//! `budget:{tenant_id}` instead — the same store `/v1/sessions` and the
//! auth verification cache already use, and exactly the old admin
//! handler's own justification for choosing this path: "no engine change
//! and no privileged Postgres role required." One consequence: unlike the
//! old system, there's no tenant registry to 404 an unknown id against —
//! any `tenant_id` string is a valid target, override or not.
//!
//! `0` in any field means "fall through to the server-global default",
//! matching the old system's JSONB shape exactly.

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::auth::{AuthContext, Role};
use crate::error::{ApiError, ApiResult};
use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/v1/tenants/{tenant_id}/budgets",
        get(get_budgets).put(put_budgets),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TenantBudgets {
    #[serde(default)]
    pub daily_tokens: u64,
    #[serde(default)]
    pub daily_llm_calls: u64,
    #[serde(default)]
    pub daily_embeddings: u64,
    #[serde(default)]
    pub monthly_tokens: u64,
    #[serde(default)]
    pub monthly_llm_calls: u64,
    #[serde(default)]
    pub monthly_embeddings: u64,
}

/// Built-in server-global caps. No config wiring yet (out of this
/// sub-slice's acceptance criteria — "admin endpoints work and are
/// covered by a test with a non-admin caller correctly rejected"); revisit
/// if a real deployment needs these tunable without a rebuild.
fn server_defaults() -> TenantBudgets {
    TenantBudgets {
        daily_tokens: 1_000_000,
        daily_llm_calls: 10_000,
        daily_embeddings: 50_000,
        monthly_tokens: 20_000_000,
        monthly_llm_calls: 200_000,
        monthly_embeddings: 1_000_000,
    }
}

/// `0` in an override slot falls through to the matching default slot.
fn effective(overrides: TenantBudgets, defaults: TenantBudgets) -> TenantBudgets {
    TenantBudgets {
        daily_tokens: nonzero_or(overrides.daily_tokens, defaults.daily_tokens),
        daily_llm_calls: nonzero_or(overrides.daily_llm_calls, defaults.daily_llm_calls),
        daily_embeddings: nonzero_or(overrides.daily_embeddings, defaults.daily_embeddings),
        monthly_tokens: nonzero_or(overrides.monthly_tokens, defaults.monthly_tokens),
        monthly_llm_calls: nonzero_or(overrides.monthly_llm_calls, defaults.monthly_llm_calls),
        monthly_embeddings: nonzero_or(overrides.monthly_embeddings, defaults.monthly_embeddings),
    }
}

fn nonzero_or(value: u64, default: u64) -> u64 {
    if value == 0 {
        default
    } else {
        value
    }
}

fn cache_key(tenant_id: &str) -> String {
    format!("budget:{tenant_id}")
}

#[derive(Debug, Serialize)]
struct BudgetsResponse {
    overrides: TenantBudgets,
    effective: TenantBudgets,
}

async fn read_overrides(state: &AppState, tenant_id: &str) -> ApiResult<TenantBudgets> {
    match state
        .cache
        .get(&cache_key(tenant_id))
        .await
        .map_err(|e| ApiError::Internal(e.into()))?
    {
        Some(raw) => serde_json::from_str(&raw).map_err(|e| ApiError::Internal(e.into())),
        None => Ok(TenantBudgets::default()),
    }
}

async fn get_budgets(
    State(state): State<AppState>,
    ctx: AuthContext,
    Path(tenant_id): Path<String>,
) -> ApiResult<Json<BudgetsResponse>> {
    ctx.require_role(&[Role::Superadmin])?;
    let overrides = read_overrides(&state, &tenant_id).await?;
    Ok(Json(BudgetsResponse {
        overrides,
        effective: effective(overrides, server_defaults()),
    }))
}

async fn put_budgets(
    State(state): State<AppState>,
    ctx: AuthContext,
    Path(tenant_id): Path<String>,
    Json(overrides): Json<TenantBudgets>,
) -> ApiResult<Json<BudgetsResponse>> {
    ctx.require_role(&[Role::Superadmin])?;
    let value = serde_json::to_string(&overrides).map_err(|e| ApiError::Internal(e.into()))?;
    state
        .cache
        .set(&cache_key(&tenant_id), value, None)
        .await
        .map_err(|e| ApiError::Internal(e.into()))?;
    Ok(Json(BudgetsResponse {
        overrides,
        effective: effective(overrides, server_defaults()),
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
            tenant_id: "acme".to_string(),
            role,
        }
    }

    #[tokio::test]
    async fn non_superadmin_caller_is_rejected() {
        for role in [Role::Admin, Role::Write, Role::Read, Role::Audit] {
            let state = test_state();
            let err = get_budgets(State(state.clone()), ctx(role), Path("acme".into()))
                .await
                .unwrap_err();
            assert!(matches!(err, ApiError::Forbidden(_)));

            let err = put_budgets(
                State(state),
                ctx(role),
                Path("acme".into()),
                Json(TenantBudgets::default()),
            )
            .await
            .unwrap_err();
            assert!(matches!(err, ApiError::Forbidden(_)));
        }
    }

    #[tokio::test]
    async fn unset_tenant_reads_all_defaults() {
        let state = test_state();
        let body = get_budgets(
            State(state),
            ctx(Role::Superadmin),
            Path("never-configured".into()),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(body.overrides, TenantBudgets::default());
        assert_eq!(body.effective, server_defaults());
    }

    #[tokio::test]
    async fn put_then_get_round_trips_and_zero_falls_through_to_default() {
        let state = test_state();
        let overrides = TenantBudgets {
            daily_tokens: 5_000_000,
            ..Default::default()
        };
        let put_body = put_budgets(
            State(state.clone()),
            ctx(Role::Superadmin),
            Path("acme".into()),
            Json(overrides),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(put_body.effective.daily_tokens, 5_000_000);
        // Every other field was left at 0 in the override, so it falls
        // through to the server default rather than reading back as 0.
        assert_eq!(
            put_body.effective.daily_llm_calls,
            server_defaults().daily_llm_calls
        );

        let get_body = get_budgets(State(state), ctx(Role::Superadmin), Path("acme".into()))
            .await
            .unwrap()
            .0;
        assert_eq!(get_body.overrides.daily_tokens, 5_000_000);
        assert_eq!(get_body.effective, put_body.effective);
    }
}
