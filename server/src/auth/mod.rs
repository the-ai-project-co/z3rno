//! JWT + API-key auth (0005.2), ported in behavior (not code) from the old
//! Python server's `middleware/auth.py`: `Authorization: Bearer <token>`
//! dispatches on whether the token has 2 dots (JWT) or not (API key).
//!
//! `AuthConfig` lives here so `AppState` (server/src/lib.rs) can hold one
//! without the rest of the crate depending on this module's internals.
//! `AuthContext`/`Role` are the contract route handlers (server/src/
//! routes/*.rs) code against via `FromRequestParts` — frozen here so
//! route handlers and the auth extractor implementation can be built in
//! parallel without either touching the other's files.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;

use crate::error::ApiError;
use crate::AppState;

mod extractor;

#[derive(Clone)]
pub struct AuthConfig {
    pub jwt_secret: String,
    /// Exactly matches the old system's `superadmin_api_key` — a request
    /// bearing this raw key gets `role = Superadmin`, the only role
    /// `require_superadmin()` (server/src/routes/admin.rs) accepts.
    /// `None` disables the superadmin surface entirely, same as the old
    /// system's `superadmin_enabled` gate.
    pub superadmin_api_key: Option<String>,
}

/// Matches the old system's four JWT roles plus the superadmin escape
/// hatch (a distinct concept from the four — see `AuthConfig::
/// superadmin_api_key`). `PartialOrd` isn't derived: role names aren't a
/// linear hierarchy in the old system (e.g. `Audit` isn't "above" `Read`),
/// so route handlers check membership in an explicit allowed-set
/// (`ctx.role.is_one_of(&[Role::Admin, Role::Write])`), never `>=`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Admin,
    Write,
    Read,
    Audit,
    Superadmin,
}

impl Role {
    pub fn is_one_of(&self, allowed: &[Role]) -> bool {
        allowed.contains(self)
    }
}

/// What a successfully authenticated request carries: which tenant it's
/// scoped to, and what it's allowed to do. Every route handler that needs
/// auth takes `ctx: AuthContext` as an extractor argument — axum runs
/// `FromRequestParts` before the handler body, so an unauthenticated or
/// under-privileged request never reaches engine/cache code at all.
#[derive(Debug, Clone)]
pub struct AuthContext {
    pub tenant_id: String,
    pub role: Role,
}

impl AuthContext {
    /// Rejects with `ApiError::Forbidden` unless `self.role` is one of
    /// `allowed` — the per-route RBAC check every protected handler opens
    /// with, matching the old system's per-route `require_role(...)`
    /// dependency (never a blanket auth gate, since `/health` etc. must
    /// stay reachable unauthenticated).
    pub fn require_role(&self, allowed: &[Role]) -> Result<(), ApiError> {
        if self.role.is_one_of(allowed) {
            Ok(())
        } else {
            Err(ApiError::Forbidden(format!(
                "role {:?} is not permitted for this operation",
                self.role
            )))
        }
    }
}

impl FromRequestParts<AppState> for AuthContext {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        extractor::authenticate(parts, state).await
    }
}
