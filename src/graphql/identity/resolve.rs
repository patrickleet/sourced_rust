//! IdentityMode resolution (D1–D14).

use axum::http::HeaderMap;

use super::oidc::{OidcConfig, OidcValidator, ValidationError};
use super::session_from_all_headers;
use crate::microsvc::{Session, ROLE_KEY, USER_ID_KEY};

/// Default identity headers stripped under TrustedProxy (fail-closed).
pub const DEFAULT_IDENTITY_STRIP_HEADERS: &[&str] = &[
    "x-user-id",
    "x-role",
    "x-roles",
    "x-hasura-user-id",
    "x-hasura-role",
    "x-hasura-allowed-roles",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IdentityMode {
    /// Mesh: strip client identity denylist (process-side defense). Gateway
    /// inject under network isolation should use Hybrid missing-Bearer path
    /// or configure gateway secret then trust.
    TrustedProxy,
    /// Public edge: Bearer JWT required when require_auth (default).
    OidcBearer,
    /// Dual path: missing Bearer → trust headers (gateway inject, F6);
    /// invalid Bearer → 401 (D2); valid → OIDC (D3).
    Hybrid,
    /// Local GraphiQL / unit tests only — ambient header trust.
    #[default]
    DevHeaders,
}

#[derive(Debug, Clone, Default)]
pub struct TrustedProxyConfig {
    pub strip_headers: Vec<String>,
    /// Optional (name, expected_value). Missing/wrong → 401 (D9).
    pub gateway_secret_header: Option<(String, String)>,
}

impl TrustedProxyConfig {
    pub fn with_defaults() -> Self {
        Self {
            strip_headers: DEFAULT_IDENTITY_STRIP_HEADERS
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
            gateway_secret_header: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct IdentityConfig {
    pub mode: IdentityMode,
    pub oidc: Option<OidcConfig>,
    pub trusted_proxy: TrustedProxyConfig,
}

impl Default for IdentityConfig {
    fn default() -> Self {
        // DevHeaders preserves existing ambient-header tests; scaffolds set OidcBearer (D6).
        Self {
            mode: IdentityMode::DevHeaders,
            oidc: None,
            trusted_proxy: TrustedProxyConfig::with_defaults(),
        }
    }
}

impl IdentityConfig {
    pub fn oidc_bearer(oidc: OidcConfig) -> Self {
        Self {
            mode: IdentityMode::OidcBearer,
            oidc: Some(oidc),
            trusted_proxy: TrustedProxyConfig::with_defaults(),
        }
    }

    pub fn hybrid(oidc: OidcConfig) -> Self {
        Self {
            mode: IdentityMode::Hybrid,
            oidc: Some(oidc),
            trusted_proxy: TrustedProxyConfig::with_defaults(),
        }
    }

    pub fn trusted_proxy() -> Self {
        Self {
            mode: IdentityMode::TrustedProxy,
            oidc: None,
            trusted_proxy: TrustedProxyConfig::with_defaults(),
        }
    }

    pub fn dev_headers() -> Self {
        Self {
            mode: IdentityMode::DevHeaders,
            oidc: None,
            trusted_proxy: TrustedProxyConfig::with_defaults(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthError {
    Unauthorized,
}

impl From<ValidationError> for AuthError {
    fn from(_: ValidationError) -> Self {
        AuthError::Unauthorized
    }
}

/// Extract Bearer token. Returns:
/// - `Ok(None)` — no Authorization or non-Bearer scheme (missing)
/// - `Ok(Some(token))` — Bearer with non-empty token
/// - `Err(Unauthorized)` — Bearer scheme with empty token (present-but-invalid)
pub fn extract_bearer(headers: &HeaderMap) -> Result<Option<String>, AuthError> {
    let Some(value) = headers.get(axum::http::header::AUTHORIZATION) else {
        return Ok(None);
    };
    let Ok(s) = value.to_str() else {
        return Err(AuthError::Unauthorized);
    };
    let s = s.trim();
    let mut parts = s.splitn(2, char::is_whitespace);
    let scheme = parts.next().unwrap_or("");
    if !scheme.eq_ignore_ascii_case("Bearer") {
        return Ok(None);
    }
    let token = parts.next().map(str::trim).unwrap_or("");
    if token.is_empty() {
        return Err(AuthError::Unauthorized);
    }
    Ok(Some(token.to_string()))
}

/// Strip identity denylist headers; keep all others.
pub fn strip_identity_headers(headers: &HeaderMap, strip: &[String]) -> Session {
    let mut vars = std::collections::HashMap::new();
    for (name, value) in headers.iter() {
        let key = name.as_str();
        if strip.iter().any(|s| s.eq_ignore_ascii_case(key)) {
            continue;
        }
        if let Ok(v) = value.to_str() {
            vars.insert(key.to_string(), v.to_string());
        }
    }
    Session::from_map(vars)
}

fn check_gateway_secret(
    headers: &HeaderMap,
    cfg: &TrustedProxyConfig,
) -> Result<(), AuthError> {
    if let Some((name, expected)) = &cfg.gateway_secret_header {
        let got = headers
            .get(name.as_str())
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if got != expected {
            return Err(AuthError::Unauthorized);
        }
    }
    Ok(())
}

fn trusted_proxy_session(headers: &HeaderMap, cfg: &TrustedProxyConfig) -> Result<Session, AuthError> {
    check_gateway_secret(headers, cfg)?;
    // Process-side strip of client identity denylist (F10).
    Ok(strip_identity_headers(headers, &cfg.strip_headers))
}

/// Hybrid missing-Bearer path: trust headers for gateway inject (F6).
/// When gateway_secret configured, still enforce it.
fn hybrid_proxy_session(headers: &HeaderMap, cfg: &TrustedProxyConfig) -> Result<Session, AuthError> {
    check_gateway_secret(headers, cfg)?;
    Ok(session_from_all_headers(headers))
}

fn oidc_session(token: &str, oidc: &OidcConfig) -> Result<Session, AuthError> {
    let validator = OidcValidator::new(oidc.clone());
    // On OIDC success, never merge client identity headers (caller passes token only).
    // Static JWKS path (tests). Live JWKS uses resolve_session async.
    validator.validate_and_map(token).map_err(Into::into)
}

async fn oidc_session_async(token: &str, oidc: &OidcConfig) -> Result<Session, AuthError> {
    let validator = OidcValidator::new(oidc.clone());
    validator
        .validate_and_map_async(token)
        .await
        .map_err(Into::into)
}

/// Resolve Session from headers + config (sync; uses static JWKS for OIDC).
///
/// For live JWKS fetch, prefer [`resolve_session`] after loading JWKS into config.static_jwks
/// or extending the validator — unit tests always use static JWKS.
pub fn resolve_session_sync(
    headers: &HeaderMap,
    config: &IdentityConfig,
) -> Result<Session, AuthError> {
    match config.mode {
        IdentityMode::DevHeaders => Ok(session_from_all_headers(headers)),
        IdentityMode::TrustedProxy => trusted_proxy_session(headers, &config.trusted_proxy),
        IdentityMode::OidcBearer => {
            let oidc = config.oidc.as_ref().ok_or(AuthError::Unauthorized)?;
            match extract_bearer(headers)? {
                None => {
                    if oidc.require_auth {
                        Err(AuthError::Unauthorized)
                    } else {
                        Ok(Session::new()) // anonymous F9
                    }
                }
                Some(token) => oidc_session(&token, oidc),
            }
        }
        IdentityMode::Hybrid => {
            let oidc = config.oidc.as_ref();
            match extract_bearer(headers)? {
                None => hybrid_proxy_session(headers, &config.trusted_proxy), // D1 / F6
                Some(token) => {
                    let oidc = oidc.ok_or(AuthError::Unauthorized)?;
                    // D2: invalid → 401, no proxy fallthrough
                    oidc_session(&token, oidc)
                }
            }
        }
    }
}

/// Resolve Session; fetches JWKS over HTTP when OIDC is configured without static JWKS.
pub async fn resolve_session(
    headers: &HeaderMap,
    config: &IdentityConfig,
) -> Result<Session, AuthError> {
    match config.mode {
        IdentityMode::OidcBearer => {
            let oidc = config.oidc.as_ref().ok_or(AuthError::Unauthorized)?;
            match extract_bearer(headers)? {
                None => {
                    if oidc.require_auth {
                        Err(AuthError::Unauthorized)
                    } else {
                        Ok(Session::new())
                    }
                }
                Some(token) => oidc_session_async(&token, oidc).await,
            }
        }
        IdentityMode::Hybrid => match extract_bearer(headers)? {
            None => hybrid_proxy_session(headers, &config.trusted_proxy),
            Some(token) => {
                let oidc = config.oidc.as_ref().ok_or(AuthError::Unauthorized)?;
                oidc_session_async(&token, oidc).await
            }
        },
        _ => resolve_session_sync(headers, config),
    }
}

/// True if session has no elevated identity (no user id / role from convenience keys).
#[allow(dead_code)]
pub fn is_anonymous_identity(session: &Session) -> bool {
    session.get(USER_ID_KEY).is_none() && session.get(ROLE_KEY).is_none()
}
