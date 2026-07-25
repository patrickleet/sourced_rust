//! JWT access-token validation + JWKS (spec token validation checklist).

use std::collections::HashMap;
use std::sync::RwLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::claims::{map_claims_to_session_with_provenance, ClaimMapConfig};
use crate::microsvc::Session;

/// OIDC validation configuration (behavior normative; field names free).
#[derive(Debug, Clone)]
pub struct OidcConfig {
    pub issuer: String,
    pub audience: String,
    /// Additional accepted audiences (e.g. multiple Keycloak confidential clients
    /// whose client_credentials tokens carry distinct `azp` values).
    pub extra_audiences: Vec<String>,
    pub jwks_uri: Option<String>,
    pub clock_skew: Duration,
    pub alg_allowlist: Vec<String>,
    /// When true (default), missing Bearer → Unauthorized.
    pub require_auth: bool,
    /// When true, valid JWT with no engine role → Unauthorized (D14 default false).
    pub require_role: bool,
    pub claim_map: ClaimMapConfig,
    /// Signed claim paths that participate in the durable command principal
    /// partition. Every configured claim must be present as a portable,
    /// non-null JSON value before a bearer identity can enter causal dispatch.
    pub principal_tenant_claims: Vec<String>,
    /// Static JWKS JSON for tests / offline (skips network).
    pub static_jwks: Option<String>,
}

impl OidcConfig {
    pub fn new(issuer: impl Into<String>, audience: impl Into<String>) -> Self {
        Self {
            issuer: issuer.into(),
            audience: audience.into(),
            extra_audiences: Vec::new(),
            jwks_uri: None,
            clock_skew: Duration::from_secs(60),
            alg_allowlist: vec!["RS256".into(), "ES256".into()],
            require_auth: true,
            require_role: false,
            claim_map: ClaimMapConfig::default(),
            principal_tenant_claims: Vec::new(),
            static_jwks: None,
        }
    }

    /// Accept additional audiences (multi-client M2M).
    pub fn with_extra_audiences(
        mut self,
        audiences: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.extra_audiences = audiences.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_static_jwks(mut self, jwks: impl Into<String>) -> Self {
        self.static_jwks = Some(jwks.into());
        self
    }

    pub fn require_auth(mut self, on: bool) -> Self {
        self.require_auth = on;
        self
    }

    pub fn engine_roles(mut self, roles: &[&str]) -> Self {
        self.claim_map.engine_roles = roles.iter().map(|s| (*s).to_string()).collect();
        self
    }

    /// Bind durable command identity to these signed tenant claim paths.
    pub fn principal_tenant_claims(
        mut self,
        claims: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.principal_tenant_claims = claims.into_iter().map(Into::into).collect();
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum VerifiedAudienceSource {
    Aud,
    AuthorizedParty,
    ClientId,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct VerifiedAudience {
    source: VerifiedAudienceSource,
    value: String,
}

/// Authentication proof admitted to durable causal dispatch.
///
/// This type is deliberately crate-private, has no public constructor, and is
/// not deserializable. A [`Session`] or trusted header map therefore cannot be
/// upgraded into a ledger principal by application or transport code.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct VerifiedPrincipal {
    issuer: String,
    subject: String,
    audiences: Vec<VerifiedAudience>,
    tenant_partitions: Vec<(String, Value)>,
}

impl VerifiedPrincipal {
    #[cfg(test)]
    pub(crate) fn test_oidc(issuer: &str, subject: &str, audiences: &[&str]) -> Self {
        assert!(
            !issuer.trim().is_empty(),
            "test OIDC issuer must not be empty"
        );
        assert!(
            !subject.trim().is_empty(),
            "test OIDC subject must not be empty"
        );
        assert!(
            !audiences.is_empty() && audiences.iter().all(|value| !value.trim().is_empty()),
            "test OIDC audiences must contain only non-empty values"
        );
        Self {
            issuer: normalize_issuer(issuer),
            subject: subject.to_string(),
            audiences: audiences
                .iter()
                .map(|value| VerifiedAudience {
                    source: VerifiedAudienceSource::Aud,
                    value: (*value).to_string(),
                })
                .collect(),
            tenant_partitions: Vec::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn issuer(&self) -> &str {
        &self.issuer
    }

    #[cfg(test)]
    pub(crate) fn subject(&self) -> &str {
        &self.subject
    }

    /// Versioned, domain-separated partition for one exact service identity.
    pub(crate) fn partition_for_service(&self, service_id: &str) -> String {
        let mut audiences = self
            .audiences
            .iter()
            .map(|audience| audience.value.as_str())
            .collect::<Vec<_>>();
        audiences.sort_unstable();
        audiences.dedup();
        let canonical = serde_json::to_vec(&serde_json::json!({
            "domain": "distributed.principal-partition",
            "version": 1,
            "service_id": service_id,
            "issuer": self.issuer,
            "subject": self.subject,
            "audiences": audiences,
            "tenant_partitions": self.tenant_partitions,
        }))
        .expect("verified principal partition values are JSON serializable");
        format!("v1:sha256:{:x}", Sha256::digest(canonical))
    }
}

impl std::fmt::Debug for VerifiedPrincipal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VerifiedPrincipal")
            .field("issuer", &self.issuer)
            .field("subject", &"[redacted]")
            .field("audiences", &"[redacted]")
            .field("tenant_partitions", &"[redacted]")
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationError {
    Malformed,
    AlgNotAllowed,
    AlgNone,
    Signature,
    Issuer,
    Audience,
    Expired,
    NotYetValid,
    MissingSub,
    TenantPartition,
    UnknownKid,
    Jwks,
    Other(String),
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Malformed => write!(f, "malformed token"),
            Self::AlgNotAllowed => write!(f, "algorithm not allowed"),
            Self::AlgNone => write!(f, "alg none rejected"),
            Self::Signature => write!(f, "signature verification failed"),
            Self::Issuer => write!(f, "issuer mismatch"),
            Self::Audience => write!(f, "audience mismatch"),
            Self::Expired => write!(f, "token expired"),
            Self::NotYetValid => write!(f, "token not yet valid"),
            Self::MissingSub => write!(f, "missing sub"),
            Self::TenantPartition => write!(f, "missing or invalid tenant partition claim"),
            Self::UnknownKid => write!(f, "unknown kid"),
            Self::Jwks => write!(f, "jwks unavailable"),
            Self::Other(s) => write!(f, "{s}"),
        }
    }
}

impl std::error::Error for ValidationError {}

#[derive(Debug, Deserialize)]
struct JwksDoc {
    keys: Vec<JwkKey>,
}

#[derive(Debug, Deserialize, Clone)]
struct JwkKey {
    kid: Option<String>,
    kty: String,
    alg: Option<String>,
    n: Option<String>,
    e: Option<String>,
    crv: Option<String>,
    x: Option<String>,
    y: Option<String>,
    #[allow(dead_code)]
    #[serde(rename = "use")]
    use_: Option<String>,
}

/// Validator with optional static JWKS and one-shot kid refresh hook.
pub struct OidcValidator {
    config: OidcConfig,
    /// kid → DecodingKey material cached from JWKS.
    cache: RwLock<HashMap<String, DecodingKey>>,
    raw_jwks: RwLock<Option<String>>,
}

impl OidcValidator {
    pub fn new(config: OidcConfig) -> Self {
        let v = Self {
            config,
            cache: RwLock::new(HashMap::new()),
            raw_jwks: RwLock::new(None),
        };
        if let Some(jwks) = &v.config.static_jwks {
            let _ = v.load_jwks_json(jwks);
        }
        v
    }

    pub fn config(&self) -> &OidcConfig {
        &self.config
    }

    pub fn load_jwks_json(&self, jwks: &str) -> Result<(), ValidationError> {
        let doc: JwksDoc = serde_json::from_str(jwks).map_err(|_| ValidationError::Jwks)?;
        let mut cache = self.cache.write().map_err(|_| ValidationError::Jwks)?;
        cache.clear();
        for key in doc.keys {
            let decoding_key = match key.kty.as_str() {
                "RSA" if jwk_alg_matches(key.alg.as_deref(), "RS256") => {
                    let (Some(n), Some(e)) = (key.n.as_deref(), key.e.as_deref()) else {
                        continue;
                    };
                    DecodingKey::from_rsa_components(n, e).map_err(|_| ValidationError::Jwks)?
                }
                "EC" if key.crv.as_deref() == Some("P-256")
                    && jwk_alg_matches(key.alg.as_deref(), "ES256") =>
                {
                    let (Some(x), Some(y)) = (key.x.as_deref(), key.y.as_deref()) else {
                        continue;
                    };
                    DecodingKey::from_ec_components(x, y).map_err(|_| ValidationError::Jwks)?
                }
                _ => continue,
            };
            let kid = key.kid.unwrap_or_else(|| "_".into());
            cache.insert(kid, decoding_key);
        }
        *self.raw_jwks.write().map_err(|_| ValidationError::Jwks)? = Some(jwks.to_string());
        Ok(())
    }

    /// Validate access-token JWT and map claims → Session.
    pub fn validate_and_map(&self, token: &str) -> Result<Session, ValidationError> {
        self.validate_and_map_principal(token)
            .map(|(session, _)| session)
    }

    /// Validate one bearer token and retain the signed identity material used
    /// by durable causal dispatch alongside the ordinary authorization session.
    pub(crate) fn validate_and_map_principal(
        &self,
        token: &str,
    ) -> Result<(Session, VerifiedPrincipal), ValidationError> {
        let claims = self.validate_token(token)?;
        let mapped = map_claims_to_session_with_provenance(&claims, &self.config.claim_map)
            .map_err(|e| {
                if e.contains("subject") {
                    ValidationError::MissingSub
                } else {
                    ValidationError::Other(e)
                }
            })?;
        if self.config.require_role && !mapped.selected_role_is_asserted {
            return Err(ValidationError::Other(
                "require_role: no asserted engine role".into(),
            ));
        }
        let principal = verified_principal(&claims, &self.config)?;
        Ok((mapped.session, principal))
    }

    /// Ensure JWKS is loaded (static or HTTP discovery / jwks_uri).
    pub async fn ensure_jwks(&self) -> Result<(), ValidationError> {
        {
            let cache = self.cache.read().map_err(|_| ValidationError::Jwks)?;
            if !cache.is_empty() {
                return Ok(());
            }
        }
        if let Some(jwks) = &self.config.static_jwks {
            return self.load_jwks_json(jwks);
        }
        let jwks_uri = match &self.config.jwks_uri {
            Some(u) => u.clone(),
            None => discover_jwks_uri(&self.config.issuer).await?,
        };
        let body = http_get_text(&jwks_uri).await?;
        self.load_jwks_json(&body)
    }

    /// Async validate with JWKS fetch when needed.
    pub async fn validate_and_map_async(&self, token: &str) -> Result<Session, ValidationError> {
        self.ensure_jwks().await?;
        self.validate_and_map(token)
    }

    pub(crate) async fn validate_and_map_principal_async(
        &self,
        token: &str,
    ) -> Result<(Session, VerifiedPrincipal), ValidationError> {
        self.ensure_jwks().await?;
        self.validate_and_map_principal(token)
    }

    /// Validate and return claims JSON (for tests).
    pub fn validate_token(&self, token: &str) -> Result<Value, ValidationError> {
        let token = token.trim();
        if token.is_empty() {
            return Err(ValidationError::Malformed);
        }

        let header = decode_header(token).map_err(|_| ValidationError::Malformed)?;
        let alg_str = match header.alg {
            Algorithm::RS256 => "RS256",
            Algorithm::ES256 => "ES256",
            Algorithm::HS256 | Algorithm::HS384 | Algorithm::HS512 => {
                return Err(ValidationError::AlgNotAllowed);
            }
            _ => {
                // jsonwebtoken may not decode "none"; check raw header.
                if let Ok(raw_alg) = raw_header_alg(token) {
                    if raw_alg.eq_ignore_ascii_case("none") {
                        return Err(ValidationError::AlgNone);
                    }
                }
                return Err(ValidationError::AlgNotAllowed);
            }
        };

        if alg_str.eq_ignore_ascii_case("none") {
            return Err(ValidationError::AlgNone);
        }
        if !self
            .config
            .alg_allowlist
            .iter()
            .any(|a| a.eq_ignore_ascii_case(alg_str))
        {
            return Err(ValidationError::AlgNotAllowed);
        }

        // Pre-check alg=none from raw header (defense if decode_header is lenient).
        if let Ok(raw_alg) = raw_header_alg(token) {
            if raw_alg.eq_ignore_ascii_case("none") {
                return Err(ValidationError::AlgNone);
            }
        }

        let kid = header.kid.unwrap_or_else(|| "_".into());
        let key = self.key_for_kid(&kid)?;

        let mut validation = Validation::new(header.alg);
        // Accept issuer with or without trailing slash (Authentik GLOBAL iss often
        // ends with `/` while config may omit it, and vice versa).
        let iss_norm = normalize_issuer(&self.config.issuer);
        let iss_slash = format!("{iss_norm}/");
        validation.set_issuer(&[&iss_norm, &iss_slash]);
        // Audience is checked manually: some IdPs (Keycloak client_credentials)
        // omit `aud` and put the client in `azp` / `client_id` instead.
        validation.validate_aud = false;
        validation.leeway = self.config.clock_skew.as_secs();
        validation.validate_exp = true;
        validation.validate_nbf = true;

        // Decode as generic JSON claims.
        let data = decode::<Value>(token, &key, &validation).map_err(map_jwt_error)?;

        let claims = data.claims;
        if verified_audiences(&claims, &self.config.audience, &self.config.extra_audiences)
            .is_none()
        {
            return Err(ValidationError::Audience);
        }
        // token_use if present
        if let Some(tu) = claims.get("token_use").and_then(|v| v.as_str()) {
            if tu != "access" {
                return Err(ValidationError::Other("token_use must be access".into()));
            }
        }

        // Ensure sub
        let sub = claims.get("sub").and_then(|v| v.as_str()).unwrap_or("");
        if sub.is_empty() {
            return Err(ValidationError::MissingSub);
        }

        Ok(claims)
    }

    fn key_for_kid(&self, kid: &str) -> Result<DecodingKey, ValidationError> {
        {
            let cache = self.cache.read().map_err(|_| ValidationError::Jwks)?;
            if let Some(k) = cache.get(kid) {
                return Ok(k.clone());
            }
            // try default key
            if let Some(k) = cache.get("_") {
                return Ok(k.clone());
            }
            if cache.len() == 1 {
                if let Some(k) = cache.values().next() {
                    return Ok(k.clone());
                }
            }
        }
        // One refresh from static jwks if present
        if let Some(jwks) = &self.config.static_jwks {
            let _ = self.load_jwks_json(jwks);
            let cache = self.cache.read().map_err(|_| ValidationError::Jwks)?;
            if let Some(k) = cache.get(kid).or_else(|| cache.values().next()) {
                return Ok(k.clone());
            }
        }
        Err(ValidationError::UnknownKid)
    }
}

fn normalize_issuer(iss: &str) -> String {
    iss.trim_end_matches('/').to_string()
}

fn jwk_alg_matches(jwk_alg: Option<&str>, expected: &str) -> bool {
    jwk_alg.is_none_or(|alg| alg.eq_ignore_ascii_case(expected))
}

/// Return the signed audience assertions that satisfied policy. Source is
/// retained for validation/audit, while principal partitioning uses the
/// canonical verified values so equivalent IdP token representations are stable.
fn verified_audiences(
    claims: &Value,
    expected: &str,
    extra: &[String],
) -> Option<Vec<VerifiedAudience>> {
    let candidates: Vec<&str> = std::iter::once(expected.trim())
        .chain(extra.iter().map(|s| s.as_str()))
        .map(str::trim)
        .filter(|candidate| !candidate.is_empty())
        .collect();
    if candidates.is_empty() {
        return None;
    }
    if let Some(aud) = claims.get("aud") {
        let asserted = match aud {
            Value::String(value) => vec![value.as_str()],
            Value::Array(values) => values.iter().filter_map(Value::as_str).collect(),
            _ => Vec::new(),
        };
        let mut matched = asserted
            .iter()
            .filter(|value| candidates.contains(value))
            .map(|value| VerifiedAudience {
                source: VerifiedAudienceSource::Aud,
                value: (*value).to_string(),
            })
            .collect::<Vec<_>>();
        matched.sort();
        matched.dedup();
        if !matched.is_empty() {
            return Some(matched);
        }

        // `aud` present but no match must not fall through to a weaker claim,
        // unless it is only the explicit empty/`account` placeholder used by
        // some client-credentials providers.
        let placeholder = match aud {
            Value::String(value) => value == "account" || value.is_empty(),
            Value::Array(values) => {
                values.is_empty()
                    || values
                        .iter()
                        .all(|value| matches!(value.as_str(), Some("account") | Some("")))
            }
            _ => false,
        };
        if !placeholder {
            return None;
        }
    }

    let mut matched = Vec::new();
    for (claim, source) in [
        ("azp", VerifiedAudienceSource::AuthorizedParty),
        ("client_id", VerifiedAudienceSource::ClientId),
    ] {
        if let Some(value) = claims.get(claim).and_then(Value::as_str) {
            if candidates.contains(&value) {
                matched.push(VerifiedAudience {
                    source,
                    value: value.to_string(),
                });
            }
        }
    }
    matched.sort();
    matched.dedup();
    (!matched.is_empty()).then_some(matched)
}

fn verified_principal(
    claims: &Value,
    config: &OidcConfig,
) -> Result<VerifiedPrincipal, ValidationError> {
    let issuer = claims
        .get("iss")
        .and_then(Value::as_str)
        .map(normalize_issuer)
        .filter(|issuer| !issuer.is_empty())
        .ok_or(ValidationError::Issuer)?;
    let subject = claims
        .get("sub")
        .and_then(Value::as_str)
        .filter(|subject| !subject.is_empty())
        .ok_or(ValidationError::MissingSub)?
        .to_string();
    let audiences = verified_audiences(claims, &config.audience, &config.extra_audiences)
        .ok_or(ValidationError::Audience)?;

    let mut claim_paths = config
        .principal_tenant_claims
        .iter()
        .map(|path| path.trim())
        .filter(|path| !path.is_empty())
        .collect::<Vec<_>>();
    claim_paths.sort_unstable();
    claim_paths.dedup();
    let mut tenant_partitions = Vec::with_capacity(claim_paths.len());
    for path in claim_paths {
        let value = claim_at_path(claims, path).ok_or(ValidationError::TenantPartition)?;
        if value.is_null() || value.as_str().is_some_and(|value| value.trim().is_empty()) {
            return Err(ValidationError::TenantPartition);
        }
        tenant_partitions.push((path.to_string(), canonical_json_value(value)));
    }

    Ok(VerifiedPrincipal {
        issuer,
        subject,
        audiences,
        tenant_partitions,
    })
}

fn claim_at_path<'a>(claims: &'a Value, path: &str) -> Option<&'a Value> {
    if path.contains('.') && !path.starts_with("urn:") {
        let mut current = claims;
        for segment in path.split('.') {
            current = current.get(segment)?;
        }
        Some(current)
    } else {
        claims.get(path)
    }
}

fn canonical_json_value(value: &Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.iter().map(canonical_json_value).collect()),
        Value::Object(values) => {
            let mut ordered = values.iter().collect::<Vec<_>>();
            ordered.sort_by_key(|(name, _)| *name);
            Value::Object(
                ordered
                    .into_iter()
                    .map(|(key, value)| (key.clone(), canonical_json_value(value)))
                    .collect(),
            )
        }
        scalar => scalar.clone(),
    }
}

fn raw_header_alg(token: &str) -> Result<String, ()> {
    let part = token.split('.').next().ok_or(())?;
    let bytes = b64url_decode(part).ok_or(())?;
    let v: Value = serde_json::from_slice(&bytes).map_err(|_| ())?;
    v.get("alg")
        .and_then(|a| a.as_str())
        .map(|s| s.to_string())
        .ok_or(())
}

fn b64url_decode(s: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    let s = s.replace('-', "+").replace('_', "/");
    let pad = match s.len() % 4 {
        2 => "==",
        3 => "=",
        _ => "",
    };
    base64::engine::general_purpose::STANDARD
        .decode(format!("{s}{pad}"))
        .ok()
}

fn map_jwt_error(e: jsonwebtoken::errors::Error) -> ValidationError {
    use jsonwebtoken::errors::ErrorKind;
    match e.kind() {
        ErrorKind::InvalidAlgorithm | ErrorKind::InvalidAlgorithmName => {
            ValidationError::AlgNotAllowed
        }
        ErrorKind::InvalidSignature => ValidationError::Signature,
        ErrorKind::InvalidIssuer => ValidationError::Issuer,
        ErrorKind::InvalidAudience => ValidationError::Audience,
        ErrorKind::ExpiredSignature => ValidationError::Expired,
        ErrorKind::ImmatureSignature => ValidationError::NotYetValid,
        ErrorKind::Base64(_) | ErrorKind::Utf8(_) | ErrorKind::Json(_) => {
            ValidationError::Malformed
        }
        _ => ValidationError::Other(e.to_string()),
    }
}

/// Current unix time for tests that craft exp manually.
#[allow(dead_code)]
pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

async fn discover_jwks_uri(issuer: &str) -> Result<String, ValidationError> {
    let base = issuer.trim_end_matches('/');
    let url = format!("{base}/.well-known/openid-configuration");
    let body = http_get_text(&url).await?;
    let v: Value = serde_json::from_str(&body).map_err(|_| ValidationError::Jwks)?;
    v.get("jwks_uri")
        .and_then(|u| u.as_str())
        .map(|s| s.to_string())
        .ok_or(ValidationError::Jwks)
}

async fn http_get_text(url: &str) -> Result<String, ValidationError> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|_| ValidationError::Jwks)?;
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|_| ValidationError::Jwks)?;
    if !resp.status().is_success() {
        return Err(ValidationError::Jwks);
    }
    resp.text().await.map_err(|_| ValidationError::Jwks)
}

#[cfg(test)]
mod principal_tests {
    use super::*;
    use serde_json::json;

    fn config() -> OidcConfig {
        OidcConfig::new("https://issuer.example/", "api-a")
            .with_extra_audiences(["api-b"])
            .principal_tenant_claims(["tenant.id", "urn:example:partition"])
    }

    fn claims(audiences: Value) -> Value {
        json!({
            "iss": "https://issuer.example",
            "sub": "subject-1",
            "aud": audiences,
            "tenant": { "id": { "region": "us", "number": 7 } },
            "urn:example:partition": ["blue", 2]
        })
    }

    #[test]
    fn principal_partition_is_stable_and_service_scoped() {
        let left = verified_principal(&claims(json!(["api-b", "api-a"])), &config()).unwrap();
        let right = verified_principal(&claims(json!(["api-a", "api-b"])), &config()).unwrap();
        assert_eq!(left, right);
        assert_eq!(
            left.partition_for_service("todos"),
            right.partition_for_service("todos")
        );
        assert_ne!(
            left.partition_for_service("todos"),
            left.partition_for_service("billing")
        );
        assert_eq!(left.issuer(), "https://issuer.example");
        assert_eq!(left.subject(), "subject-1");
    }

    #[test]
    fn equivalent_verified_audience_sources_share_partition_identity() {
        let aud = verified_principal(&claims(json!("api-a")), &config()).unwrap();
        let mut fallback_claims = claims(json!("account"));
        fallback_claims["azp"] = json!("api-a");
        let azp = verified_principal(&fallback_claims, &config()).unwrap();
        assert_eq!(
            aud.partition_for_service("todos"),
            azp.partition_for_service("todos")
        );
    }

    #[test]
    fn role_changes_do_not_create_a_new_principal_partition() {
        let mut user = claims(json!("api-a"));
        user["role"] = json!("user");
        let mut admin = claims(json!("api-a"));
        admin["role"] = json!("admin");
        assert_eq!(
            verified_principal(&user, &config())
                .unwrap()
                .partition_for_service("todos"),
            verified_principal(&admin, &config())
                .unwrap()
                .partition_for_service("todos")
        );
    }

    #[test]
    fn configured_tenant_claim_is_required_and_cannot_be_null() {
        let mut missing = claims(json!("api-a"));
        missing.as_object_mut().unwrap().remove("tenant");
        assert_eq!(
            verified_principal(&missing, &config()).unwrap_err(),
            ValidationError::TenantPartition
        );

        let mut null = claims(json!("api-a"));
        null["tenant"]["id"] = Value::Null;
        assert_eq!(
            verified_principal(&null, &config()).unwrap_err(),
            ValidationError::TenantPartition
        );

        let mut blank = claims(json!("api-a"));
        blank["tenant"]["id"] = json!("   ");
        assert_eq!(
            verified_principal(&blank, &config()).unwrap_err(),
            ValidationError::TenantPartition
        );
    }

    #[test]
    fn principal_debug_redacts_subject_and_tenant_values() {
        let principal = verified_principal(&claims(json!("api-a")), &config()).unwrap();
        let debug = format!("{principal:?}");
        assert!(!debug.contains("subject-1"));
        assert!(!debug.contains("api-a"));
        assert!(!debug.contains("region"));
        assert!(debug.contains("[redacted]"));
    }
}
