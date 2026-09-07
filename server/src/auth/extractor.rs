//! Implements `authenticate()`, the function `AuthContext`'s
//! `FromRequestParts` impl (`super`) delegates to. Ported in behavior from
//! the old Python `middleware/auth.py`: read `Authorization: Bearer
//! <token>`, then dispatch on the token's shape — exactly 2 `.` characters
//! means a JWT (`header.payload.signature`), anything else means an API
//! key. That heuristic is the old system's, kept verbatim because it's
//! simple and correct.
//!
//! Two credential kinds share the API-key branch:
//!   - the superadmin key (`AuthConfig::superadmin_api_key`), a
//!     pre-shared operator secret checked by plain string equality; and
//!   - an ordinary API key, looked up as `sha256(raw_key)` in
//!     `CacheBackend` under `apikey:<hex>` -> `"<tenant_id>|<role>"`.
//!     The hash (not the raw key) is the cache key so a cache dump
//!     doesn't hand out working credentials. `sha2` is used rather than
//!     `bcrypt` here on purpose: bcrypt is for verifying a password
//!     against its own stored hash (slow by design, one direction), but
//!     this is a deterministic lookup key into a cache we control —
//!     there's no stored-hash comparison to make bcrypt's slowness buy
//!     anything, it would just add CPU cost with no security benefit.
//!
//! **Gap, not a placeholder**: nothing in this sub-slice *writes*
//! `apikey:*` cache entries — there's no key-issuance endpoint yet (the
//! old system had an `api_keys` Postgres table this rewrite doesn't have
//! an equivalent of). The lookup mechanism below is real and correct; it
//! will simply 401 every ordinary API key until an issuance endpoint
//! populates the cache. That endpoint is out of scope here.
//!
//! **No verification-result cache**: the old system cached successful
//! auth lookups in Redis because Python's bcrypt check was CPU-bound.
//! Nothing on this path is: `jsonwebtoken`'s HMAC verify is cheap, and
//! the API-key path is already exactly one cache round trip — caching in
//! front of a cache lookup buys nothing but complexity. Skipped on
//! purpose; revisit only if profiling says otherwise.

use std::fmt::Write as _;

use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{AuthConfig, AuthContext, Role};
use crate::cache::CacheBackend;
use crate::error::ApiError;
use crate::AppState;

pub async fn authenticate(parts: &mut Parts, state: &AppState) -> Result<AuthContext, ApiError> {
    let header = parts
        .headers
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    authenticate_header(header, &state.auth, state.cache.as_ref()).await
}

/// Claims this server requires. `sub` must be present but its value is
/// never read beyond that — it's the caller's identity, not something
/// this server makes decisions on. `iat` is accepted if present but not
/// required or checked; `exp` is required so `jsonwebtoken`'s default
/// `Validation` (which always checks it) has something to check against —
/// a token missing it fails to decode, same as an expired one.
#[derive(Debug, Deserialize, Serialize)]
struct Claims {
    #[allow(dead_code)]
    sub: String,
    org_id: String,
    role: String,
    exp: usize,
    #[allow(dead_code)]
    iat: Option<usize>,
}

/// The header-string-in, `AuthContext`-or-error-out core, factored out of
/// `authenticate` so tests can drive it without constructing a real
/// `axum::http::request::Parts`.
async fn authenticate_header(
    header: Option<&str>,
    auth: &AuthConfig,
    cache: &dyn CacheBackend,
) -> Result<AuthContext, ApiError> {
    let header =
        header.ok_or_else(|| ApiError::Unauthorized("missing Authorization header".into()))?;
    let token = header.strip_prefix("Bearer ").ok_or_else(|| {
        ApiError::Unauthorized("Authorization header must be 'Bearer <token>'".into())
    })?;

    // JWTs are always header.payload.signature — exactly 2 dots. Anything
    // else is an API key. Ported verbatim from the old system.
    if token.matches('.').count() == 2 {
        authenticate_jwt(token, &auth.jwt_secret)
    } else {
        authenticate_api_key(token, auth, cache).await
    }
}

fn authenticate_jwt(token: &str, jwt_secret: &str) -> Result<AuthContext, ApiError> {
    let key = DecodingKey::from_secret(jwt_secret.as_bytes());
    // Default Validation: algorithm HS256 only, exp checked, no
    // audience/issuer requirement (this server doesn't set either).
    let data = decode::<Claims>(token, &key, &Validation::new(Algorithm::HS256))
        // Never leak *why* (bad signature vs expired vs malformed vs
        // missing claim) — a real security practice worth keeping from
        // the old system.
        .map_err(|_| ApiError::Unauthorized("invalid or expired token".into()))?;

    let role = map_role(&data.claims.role)
        .ok_or_else(|| ApiError::Unauthorized("invalid or expired token".into()))?;

    Ok(AuthContext {
        tenant_id: data.claims.org_id,
        role,
    })
}

async fn authenticate_api_key(
    token: &str,
    auth: &AuthConfig,
    cache: &dyn CacheBackend,
) -> Result<AuthContext, ApiError> {
    // Pre-shared operator secret, plain string comparison — not a
    // hashed/looked-up value, matching the old system exactly. A
    // superadmin request has no meaningful single tenant_id (it acts
    // across tenants, not within one): the one route that consumes
    // `Role::Superadmin` (admin/budgets) takes the target tenant from the
    // URL path instead of `ctx.tenant_id`.
    if let Some(expected) = &auth.superadmin_api_key {
        if token == expected {
            return Ok(AuthContext {
                tenant_id: String::new(),
                role: Role::Superadmin,
            });
        }
    }

    let cache_key = format!("apikey:{}", sha256_hex(token));
    let entry = cache
        .get(&cache_key)
        .await
        .map_err(anyhow::Error::from)?
        .ok_or_else(|| ApiError::Unauthorized("invalid API key".into()))?;

    let (tenant_id, role_str) = entry
        .split_once('|')
        .ok_or_else(|| ApiError::Unauthorized("invalid API key".into()))?;
    let role =
        map_role(role_str).ok_or_else(|| ApiError::Unauthorized("invalid API key".into()))?;

    Ok(AuthContext {
        tenant_id: tenant_id.to_string(),
        role,
    })
}

/// `superadmin` deliberately maps to `None` here — it's only ever granted
/// via the API-key path (`AuthConfig::superadmin_api_key`), never claimed
/// by a JWT or an ordinary API-key cache entry.
fn map_role(s: &str) -> Option<Role> {
    match s {
        "admin" => Some(Role::Admin),
        "write" => Some(Role::Write),
        "read" => Some(Role::Read),
        "audit" => Some(Role::Audit),
        _ => None,
    }
}

/// Lower-case hex SHA-256, hand-rolled rather than pulling in a `hex`
/// crate for one call site.
fn sha256_hex(input: &str) -> String {
    let digest = Sha256::digest(input.as_bytes());
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(out, "{byte:02x}").expect("writing to a String never fails");
    }
    out
}

#[cfg(test)]
mod tests {
    use jsonwebtoken::{encode, EncodingKey, Header};

    use super::*;
    use crate::cache::SqliteCacheBackend;

    const SECRET: &str = "test-jwt-secret";
    const SUPERADMIN_KEY: &str = "super-secret-operator-key";

    fn auth_config() -> AuthConfig {
        AuthConfig {
            jwt_secret: SECRET.to_string(),
            superadmin_api_key: Some(SUPERADMIN_KEY.to_string()),
        }
    }

    fn make_jwt(org_id: &str, role: &str, exp_offset_secs: i64) -> String {
        let exp = (chrono::Utc::now().timestamp() + exp_offset_secs) as usize;
        let claims = Claims {
            sub: "user-1".to_string(),
            org_id: org_id.to_string(),
            role: role.to_string(),
            exp,
            iat: Some(chrono::Utc::now().timestamp() as usize),
        };
        encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(SECRET.as_bytes()),
        )
        .unwrap()
    }

    async fn cache() -> SqliteCacheBackend {
        SqliteCacheBackend::open_in_memory().unwrap()
    }

    #[tokio::test]
    async fn missing_header_is_unauthorized() {
        let cache = cache().await;
        let err = authenticate_header(None, &auth_config(), &cache)
            .await
            .unwrap_err();
        assert!(matches!(err, ApiError::Unauthorized(_)));
    }

    #[tokio::test]
    async fn malformed_bearer_prefix_is_unauthorized() {
        let cache = cache().await;
        let err = authenticate_header(Some("Basic dXNlcjpwYXNz"), &auth_config(), &cache)
            .await
            .unwrap_err();
        assert!(matches!(err, ApiError::Unauthorized(_)));
    }

    #[tokio::test]
    async fn valid_jwt_maps_each_role() {
        let cache = cache().await;
        let cfg = auth_config();
        for (role_str, expected) in [
            ("admin", Role::Admin),
            ("write", Role::Write),
            ("read", Role::Read),
            ("audit", Role::Audit),
        ] {
            let token = make_jwt("tenant-a", role_str, 3600);
            let header = format!("Bearer {token}");
            let ctx = authenticate_header(Some(&header), &cfg, &cache)
                .await
                .unwrap();
            assert_eq!(ctx.tenant_id, "tenant-a");
            assert_eq!(ctx.role, expected);
        }
    }

    #[tokio::test]
    async fn expired_jwt_is_unauthorized() {
        let cache = cache().await;
        let token = make_jwt("tenant-a", "admin", -3600);
        let header = format!("Bearer {token}");
        let err = authenticate_header(Some(&header), &auth_config(), &cache)
            .await
            .unwrap_err();
        assert!(matches!(err, ApiError::Unauthorized(_)));
    }

    #[tokio::test]
    async fn jwt_with_unmapped_role_is_unauthorized() {
        let cache = cache().await;
        let cfg = auth_config();
        for bad_role in ["superadmin", "bogus"] {
            let token = make_jwt("tenant-a", bad_role, 3600);
            let header = format!("Bearer {token}");
            let err = authenticate_header(Some(&header), &cfg, &cache)
                .await
                .unwrap_err();
            assert!(matches!(err, ApiError::Unauthorized(_)));
        }
    }

    #[tokio::test]
    async fn correct_superadmin_key_grants_superadmin_role() {
        let cache = cache().await;
        let header = format!("Bearer {SUPERADMIN_KEY}");
        let ctx = authenticate_header(Some(&header), &auth_config(), &cache)
            .await
            .unwrap();
        assert_eq!(ctx.role, Role::Superadmin);
        assert_eq!(ctx.tenant_id, "");
    }

    #[tokio::test]
    async fn non_jwt_garbage_is_unauthorized_not_panic() {
        let cache = cache().await;
        let cfg = auth_config();
        for bad in ["no-dots-at-all", "one.dot", "three.dots.here.nope"] {
            let header = format!("Bearer {bad}");
            let result = authenticate_header(Some(&header), &cfg, &cache).await;
            // "three.dots.here.nope" has 3 dots so it also lands on the
            // API-key path (only exactly-2-dots is treated as a JWT) —
            // still must reject cleanly, not panic.
            assert!(matches!(result, Err(ApiError::Unauthorized(_))));
        }
    }

    #[tokio::test]
    async fn registered_api_key_is_looked_up_via_cache() {
        let cache = cache().await;
        let raw_key = "sk_live_abc123";
        let hash = sha256_hex(raw_key);
        cache
            .set(
                &format!("apikey:{hash}"),
                "tenant-b|write".to_string(),
                None,
            )
            .await
            .unwrap();

        let header = format!("Bearer {raw_key}");
        let ctx = authenticate_header(Some(&header), &auth_config(), &cache)
            .await
            .unwrap();
        assert_eq!(ctx.tenant_id, "tenant-b");
        assert_eq!(ctx.role, Role::Write);
    }

    #[tokio::test]
    async fn unregistered_api_key_is_unauthorized() {
        let cache = cache().await;
        let header = "Bearer sk_never_registered";
        let err = authenticate_header(Some(header), &auth_config(), &cache)
            .await
            .unwrap_err();
        assert!(matches!(err, ApiError::Unauthorized(_)));
    }
}
