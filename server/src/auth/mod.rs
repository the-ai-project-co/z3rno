//! JWT + API-key auth (0005.2), ported in behavior (not code) from the old
//! Python server's `middleware/auth.py`: `Authorization: Bearer <token>`
//! dispatches on whether the token has 2 dots (JWT) or not (API key).
//!
//! `AuthConfig` lives here so `AppState` (server/src/lib.rs) can hold one
//! without the rest of the crate depending on this module's internals.

#[derive(Clone)]
pub struct AuthConfig {
    pub jwt_secret: String,
    /// Exactly matches the old system's `superadmin_api_key` — a request
    /// bearing this raw key gets `role = "superadmin"`, the only role
    /// `require_superadmin()` (server/src/routes/admin.rs) accepts.
    /// `None` disables the superadmin surface entirely, same as the old
    /// system's `superadmin_enabled` gate.
    pub superadmin_api_key: Option<String>,
}
