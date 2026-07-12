//! JWT access-token validation + JWKS (spec token validation checklist).

use std::collections::HashMap;
use std::sync::RwLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use jsonwebtoken::{
    decode, decode_header, Algorithm, DecodingKey, Validation,
};
use serde::Deserialize;
use serde_json::Value;

use super::claims::{map_claims_to_session, ClaimMapConfig};
use crate::microsvc::Session;

/// OIDC validation configuration (behavior normative; field names free).
#[derive(Debug, Clone)]
pub struct OidcConfig {
    pub issuer: String,
    pub audience: String,
    pub jwks_uri: Option<String>,
    pub clock_skew: Duration,
    pub alg_allowlist: Vec<String>,
    /// When true (default), missing Bearer → Unauthorized.
    pub require_auth: bool,
    /// When true, valid JWT with no engine role → Unauthorized (D14 default false).
    pub require_role: bool,
    pub claim_map: ClaimMapConfig,
    /// Static JWKS JSON for tests / offline (skips network).
    pub static_jwks: Option<String>,
}

impl OidcConfig {
    pub fn new(issuer: impl Into<String>, audience: impl Into<String>) -> Self {
        Self {
            issuer: issuer.into(),
            audience: audience.into(),
            jwks_uri: None,
            clock_skew: Duration::from_secs(60),
            alg_allowlist: vec!["RS256".into(), "ES256".into()],
            require_auth: true,
            require_role: false,
            claim_map: ClaimMapConfig::default(),
            static_jwks: None,
        }
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
    #[allow(dead_code)]
    alg: Option<String>,
    n: Option<String>,
    e: Option<String>,
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
            if key.kty != "RSA" {
                continue;
            }
            let (Some(n), Some(e)) = (key.n.as_deref(), key.e.as_deref()) else {
                continue;
            };
            let dk = DecodingKey::from_rsa_components(n, e).map_err(|_| ValidationError::Jwks)?;
            let kid = key.kid.unwrap_or_else(|| "_".into());
            cache.insert(kid, dk);
        }
        *self.raw_jwks.write().map_err(|_| ValidationError::Jwks)? = Some(jwks.to_string());
        Ok(())
    }

    /// Validate access-token JWT and map claims → Session.
    pub fn validate_and_map(&self, token: &str) -> Result<Session, ValidationError> {
        let claims = self.validate_token(token)?;
        let session = map_claims_to_session(&claims, &self.config.claim_map).map_err(|e| {
            if e.contains("subject") {
                ValidationError::MissingSub
            } else {
                ValidationError::Other(e)
            }
        })?;
        if self.config.require_role && session.role().is_none() {
            return Err(ValidationError::Other("require_role: no engine role".into()));
        }
        Ok(session)
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
        validation.set_issuer(&[normalize_issuer(&self.config.issuer)]);
        validation.set_audience(&[&self.config.audience]);
        validation.leeway = self.config.clock_skew.as_secs();
        validation.validate_exp = true;
        validation.validate_nbf = true;

        // Decode as generic JSON claims.
        let data = decode::<Value>(token, &key, &validation).map_err(|e| map_jwt_error(e))?;

        let claims = data.claims;
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
        ErrorKind::Base64(_) | ErrorKind::Utf8(_) | ErrorKind::Json(_) => ValidationError::Malformed,
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
