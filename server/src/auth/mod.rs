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

/// Claims shape signed by `issue_jwt` below — kept in exact sync with
/// `extractor::authenticate_jwt`'s own private `Claims` (`sub`, `org_id`,
/// `role`, `exp`, `iat`) by hand, since the two aren't allowed to share a
/// type: `extractor`'s stays private so nothing outside this module can
/// forge a claim without going through the one function here that's
/// actually meant to mint one.
#[derive(serde::Serialize)]
struct IssuedClaims {
    sub: String,
    org_id: String,
    role: String,
    exp: usize,
    iat: usize,
}

/// Signs a JWT this server's own auth extractor will accept for
/// `tenant_id` with `role`, valid for `ttl_secs` from now. This is the
/// self-service credential-issuance path #35 asked for: no database-backed
/// API-key system, just the CLI (`z3rno token`) minting a token with the
/// same secret the server verifies against — the same shared-secret model
/// `--jwt-secret`/`Z3RNO_JWT_SECRET` already implies. `Role::Superadmin`
/// isn't issuable this way (it's a pre-shared operator key, not a JWT
/// claim — see `AuthConfig::superadmin_api_key`).
pub fn issue_jwt(
    jwt_secret: &str,
    tenant_id: &str,
    role: Role,
    ttl_secs: i64,
) -> anyhow::Result<String> {
    let role_str = match role {
        Role::Admin => "admin",
        Role::Write => "write",
        Role::Read => "read",
        Role::Audit => "audit",
        Role::Superadmin => {
            anyhow::bail!("superadmin isn't issuable as a JWT — use --superadmin-api-key instead")
        }
    };
    let now = chrono::Utc::now().timestamp();
    let claims = IssuedClaims {
        sub: format!("cli-issued:{tenant_id}"),
        org_id: tenant_id.to_string(),
        role: role_str.to_string(),
        exp: (now + ttl_secs.max(0)) as usize,
        iat: now as usize,
    };
    let token = jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(jwt_secret.as_bytes()),
    )?;
    Ok(token)
}

#[cfg(test)]
mod issue_jwt_tests {
    use super::*;

    #[test]
    fn superadmin_is_rejected() {
        assert!(issue_jwt("secret", "tenant-a", Role::Superadmin, 3600).is_err());
    }
}
