//! IdentityMode resolution (D1–D14).

use axum::http::HeaderMap;

use super::oidc::{OidcConfig, OidcValidator, ValidationError, VerifiedPrincipal};
use super::session_from_all_headers;
use crate::microsvc::{Session, USER_ID_KEY};

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

/// Reusable request identity resolver with one shared OIDC validator and JWKS cache.
///
/// Construct one resolver per service or middleware layer and reuse it across
/// requests so live JWKS discovery, parsing, and refresh state remain cached.
pub struct IdentityResolver {
    config: IdentityConfig,
    validator: Option<OidcValidator>,
}

impl IdentityResolver {
    /// Build a resolver and its shared validator from one identity configuration.
    pub fn new(config: IdentityConfig) -> Self {
        let validator = match config.mode {
            IdentityMode::OidcBearer | IdentityMode::Hybrid => {
                config.oidc.clone().map(OidcValidator::new)
            }
            IdentityMode::DevHeaders | IdentityMode::TrustedProxy => None,
        };
        Self { config, validator }
    }

    /// Return the immutable identity configuration used for every resolution.
    pub fn config(&self) -> &IdentityConfig {
        &self.config
    }

    /// Resolve one request while reusing this resolver's live JWKS cache.
    pub async fn resolve_session(&self, headers: &HeaderMap) -> Result<Session, AuthError> {
        self.resolve_identity(headers)
            .await
            .map(|identity| identity.session)
    }

    pub(crate) async fn resolve_identity(
        &self,
        headers: &HeaderMap,
    ) -> Result<ResolvedIdentity, AuthError> {
        resolve_identity_with_validator(headers, &self.config, self.validator.as_ref()).await
    }
}

/// Placeholder issuer/audience when OIDC env is unset — fail-closed (401) until configured.
pub const UNSET_OIDC_ISSUER: &str = "http://localhost/unset-oidc-issuer";
pub const UNSET_OIDC_AUDIENCE: &str = "unset-audience";

/// Public GraphQL scaffold identity (D6/D7): always **`OidcBearer`** + `require_auth=true`.
///
/// Pure inputs so tests do not mutate process env. See [`public_oidc_identity_from_env`].
///
/// - When `issuer` and `audience` (or `client_id` as audience fallback) are non-empty → use them.
/// - When unset → placeholder issuer/audience so requests still require Bearer and reject
///   ambient `x-user-id` / `x-roles` (never [`IdentityMode::DevHeaders`]).
pub fn public_oidc_identity_from_env_vars(
    issuer: Option<&str>,
    audience: Option<&str>,
    client_id: Option<&str>,
    jwks_uri: Option<&str>,
) -> IdentityConfig {
    let iss = issuer
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(UNSET_OIDC_ISSUER);
    let aud = audience
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| client_id.map(str::trim).filter(|s| !s.is_empty()))
        .unwrap_or(UNSET_OIDC_AUDIENCE);

    let mut oidc = OidcConfig::new(iss, aud);
    oidc.require_auth = true;
    if let Some(jwks) = jwks_uri.map(str::trim).filter(|s| !s.is_empty()) {
        oidc.jwks_uri = Some(jwks.to_string());
    }
    IdentityConfig::oidc_bearer(oidc)
}

/// Read process env (`OIDC_ISSUER`, `OIDC_AUDIENCE` / `OIDC_CLIENT_ID`, `OIDC_JWKS_URI`)
/// and apply [`public_oidc_identity_from_env_vars`]. Always OidcBearer (D6).
pub fn public_oidc_identity_from_env() -> IdentityConfig {
    public_oidc_identity_from_env_vars(
        std::env::var("OIDC_ISSUER").ok().as_deref(),
        std::env::var("OIDC_AUDIENCE").ok().as_deref(),
        std::env::var("OIDC_CLIENT_ID").ok().as_deref(),
        std::env::var("OIDC_JWKS_URI").ok().as_deref(),
    )
}

#[cfg(test)]
mod public_default_tests {
    use super::*;

    #[test]
    fn d6_unset_env_is_oidc_bearer_not_dev_headers() {
        let cfg = public_oidc_identity_from_env_vars(None, None, None, None);
        assert_eq!(cfg.mode, IdentityMode::OidcBearer);
        assert_ne!(cfg.mode, IdentityMode::DevHeaders);
        let oidc = cfg.oidc.as_ref().expect("oidc config");
        assert!(oidc.require_auth);
        assert_eq!(oidc.issuer, UNSET_OIDC_ISSUER);
        assert_eq!(oidc.audience, UNSET_OIDC_AUDIENCE);
    }

    #[test]
    fn d6_configured_env_uses_issuer_audience() {
        let cfg = public_oidc_identity_from_env_vars(
            Some("http://localhost:8080"),
            Some("graphql-api"),
            None,
            Some("http://localhost:8080/oauth/v2/keys"),
        );
        assert_eq!(cfg.mode, IdentityMode::OidcBearer);
        let oidc = cfg.oidc.as_ref().unwrap();
        assert!(oidc.require_auth);
        assert_eq!(oidc.issuer, "http://localhost:8080");
        assert_eq!(oidc.audience, "graphql-api");
        assert_eq!(
            oidc.jwks_uri.as_deref(),
            Some("http://localhost:8080/oauth/v2/keys")
        );
    }

    #[test]
    fn d6_client_id_falls_back_as_audience() {
        let cfg =
            public_oidc_identity_from_env_vars(Some("http://iss"), None, Some("client-123"), None);
        assert_eq!(cfg.mode, IdentityMode::OidcBearer);
        assert_eq!(cfg.oidc.as_ref().unwrap().audience, "client-123");
    }

    #[test]
    fn d6_unset_rejects_ambient_headers_via_resolve() {
        use axum::http::{HeaderMap, HeaderValue};
        let cfg = public_oidc_identity_from_env_vars(None, None, None, None);
        let mut headers = HeaderMap::new();
        headers.insert("x-user-id", HeaderValue::from_static("attacker"));
        headers.insert("x-roles", HeaderValue::from_static("admin"));
        // No Bearer → require_auth → Unauthorized (not DevHeaders trust)
        assert_eq!(
            resolve_session_sync(&headers, &cfg).unwrap_err(),
            AuthError::Unauthorized
        );
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthError {
    Unauthorized,
}

/// A normal authorization session plus the optional bearer-only proof required
/// for durable causal command dispatch.
pub(crate) struct ResolvedIdentity {
    session: Session,
    principal: Option<VerifiedPrincipal>,
}

impl ResolvedIdentity {
    pub(crate) fn unverified(session: Session) -> Self {
        Self {
            session,
            principal: None,
        }
    }

    fn verified(session: Session, principal: VerifiedPrincipal) -> Self {
        Self {
            session,
            principal: Some(principal),
        }
    }

    pub(crate) fn into_parts(self) -> (Session, Option<VerifiedPrincipal>) {
        (self.session, self.principal)
    }
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

fn check_gateway_secret(headers: &HeaderMap, cfg: &TrustedProxyConfig) -> Result<(), AuthError> {
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

fn trusted_proxy_session(
    headers: &HeaderMap,
    cfg: &TrustedProxyConfig,
) -> Result<Session, AuthError> {
    check_gateway_secret(headers, cfg)?;
    // Process-side strip of client identity denylist (F10).
    Ok(strip_identity_headers(headers, &cfg.strip_headers))
}

/// Hybrid missing-Bearer path: trust headers for gateway inject (F6).
/// When gateway_secret configured, still enforce it.
fn hybrid_proxy_session(
    headers: &HeaderMap,
    cfg: &TrustedProxyConfig,
) -> Result<Session, AuthError> {
    check_gateway_secret(headers, cfg)?;
    Ok(session_from_all_headers(headers))
}

fn oidc_identity(token: &str, oidc: &OidcConfig) -> Result<ResolvedIdentity, AuthError> {
    let validator = OidcValidator::new(oidc.clone());
    // On OIDC success, never merge client identity headers (caller passes token only).
    // Static JWKS path (tests). Live JWKS uses resolve_identity async.
    let (session, principal) = validator
        .validate_and_map_principal(token)
        .map_err(AuthError::from)?;
    Ok(ResolvedIdentity::verified(session, principal))
}

async fn oidc_identity_async(
    token: &str,
    validator: &OidcValidator,
) -> Result<ResolvedIdentity, AuthError> {
    let (session, principal) = validator
        .validate_and_map_principal_async(token)
        .await
        .map_err(AuthError::from)?;
    Ok(ResolvedIdentity::verified(session, principal))
}

/// Resolve Session from headers + config (sync; uses static JWKS for OIDC).
///
/// For live JWKS fetch, prefer [`resolve_session`] after loading JWKS into config.static_jwks
/// or extending the validator — unit tests always use static JWKS.
pub fn resolve_session_sync(
    headers: &HeaderMap,
    config: &IdentityConfig,
) -> Result<Session, AuthError> {
    resolve_identity_sync(headers, config).map(|identity| identity.session)
}

pub(crate) fn resolve_identity_sync(
    headers: &HeaderMap,
    config: &IdentityConfig,
) -> Result<ResolvedIdentity, AuthError> {
    match config.mode {
        IdentityMode::DevHeaders => Ok(ResolvedIdentity::unverified(session_from_all_headers(
            headers,
        ))),
        IdentityMode::TrustedProxy => {
            trusted_proxy_session(headers, &config.trusted_proxy).map(ResolvedIdentity::unverified)
        }
        IdentityMode::OidcBearer => {
            let oidc = config.oidc.as_ref().ok_or(AuthError::Unauthorized)?;
            match extract_bearer(headers)? {
                None => {
                    if oidc.require_auth {
                        Err(AuthError::Unauthorized)
                    } else {
                        Ok(ResolvedIdentity::unverified(Session::new())) // anonymous F9
                    }
                }
                Some(token) => oidc_identity(&token, oidc),
            }
        }
        IdentityMode::Hybrid => {
            let oidc = config.oidc.as_ref();
            match extract_bearer(headers)? {
                None => hybrid_proxy_session(headers, &config.trusted_proxy)
                    .map(ResolvedIdentity::unverified), // D1 / F6
                Some(token) => {
                    let oidc = oidc.ok_or(AuthError::Unauthorized)?;
                    // D2: invalid → 401, no proxy fallthrough
                    oidc_identity(&token, oidc)
                }
            }
        }
    }
}

/// Resolve one Session, fetching JWKS when needed.
///
/// Services handling repeated requests should reuse [`IdentityResolver`] so
/// live JWKS remain cached between calls.
pub async fn resolve_session(
    headers: &HeaderMap,
    config: &IdentityConfig,
) -> Result<Session, AuthError> {
    IdentityResolver::new(config.clone())
        .resolve_session(headers)
        .await
}

pub(crate) async fn resolve_identity_with_validator(
    headers: &HeaderMap,
    config: &IdentityConfig,
    validator: Option<&OidcValidator>,
) -> Result<ResolvedIdentity, AuthError> {
    match config.mode {
        IdentityMode::OidcBearer => {
            let oidc = config.oidc.as_ref().ok_or(AuthError::Unauthorized)?;
            match extract_bearer(headers)? {
                None => {
                    if oidc.require_auth {
                        Err(AuthError::Unauthorized)
                    } else {
                        Ok(ResolvedIdentity::unverified(Session::new()))
                    }
                }
                Some(token) => {
                    let validator = validator.ok_or(AuthError::Unauthorized)?;
                    oidc_identity_async(&token, validator).await
                }
            }
        }
        IdentityMode::Hybrid => match extract_bearer(headers)? {
            None => hybrid_proxy_session(headers, &config.trusted_proxy)
                .map(ResolvedIdentity::unverified),
            Some(token) => {
                let _ = config.oidc.as_ref().ok_or(AuthError::Unauthorized)?;
                let validator = validator.ok_or(AuthError::Unauthorized)?;
                oidc_identity_async(&token, validator).await
            }
        },
        _ => resolve_identity_sync(headers, config),
    }
}

/// True if session has no elevated identity (no user id / asserted roles).
#[allow(dead_code)]
pub fn is_anonymous_identity(session: &Session) -> bool {
    session.get(USER_ID_KEY).is_none() && session.roles().is_empty()
}

#[cfg(test)]
mod causal_identity_tests {
    use super::*;
    use axum::http::{HeaderMap, HeaderValue};

    #[test]
    fn ambient_and_proxy_identity_never_mint_a_causal_principal() {
        let mut headers = HeaderMap::new();
        headers.insert(USER_ID_KEY, HeaderValue::from_static("spoofed-user"));
        headers.insert("x-roles", HeaderValue::from_static("admin"));

        for config in [
            IdentityConfig::dev_headers(),
            IdentityConfig::trusted_proxy(),
            IdentityConfig::hybrid(OidcConfig::new("https://issuer", "api")),
        ] {
            let identity = resolve_identity_sync(&headers, &config).unwrap();
            let (_, principal) = identity.into_parts();
            assert!(principal.is_none(), "mode {:?} minted a proof", config.mode);
        }
    }

    #[test]
    fn anonymous_oidc_mode_has_no_causal_principal_without_a_bearer() {
        let config = IdentityConfig::oidc_bearer(
            OidcConfig::new("https://issuer", "api").require_auth(false),
        );
        let identity = resolve_identity_sync(&HeaderMap::new(), &config).unwrap();
        let (session, principal) = identity.into_parts();
        assert!(is_anonymous_identity(&session));
        assert!(principal.is_none());
    }
}
