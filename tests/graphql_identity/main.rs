//! Always-on identity suite: fixtures F1–F10 against shipped identity path.
//!
//! No network / no Zitadel. Synthetic RSA JWKS + JWT via jsonwebtoken.

#![cfg(all(feature = "graphql", feature = "sqlite"))]

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::http::{HeaderMap, HeaderValue};
use base64::Engine;
use distributed::graphql::{
    extract_bearer, graphql_router, map_claims_to_session, resolve_session_sync, select,
    strip_identity_headers, AuthError, ClaimMapConfig, GraphqlEngine, IdentityConfig,
    IdentityMode, ModelPermissions, OidcConfig, OidcValidator, DEFAULT_IDENTITY_STRIP_HEADERS,
};
use distributed::ReadModel;
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use rsa::pkcs1::EncodeRsaPrivateKey;
use rsa::traits::PublicKeyParts;
use rsa::{RsaPrivateKey, RsaPublicKey};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::sqlite::SqlitePoolOptions;
use tower::util::ServiceExt;

// ── RSA fixture helpers ─────────────────────────────────────────────────────

struct TestKeys {
    encoding: EncodingKey,
    jwks_json: String,
    #[allow(dead_code)]
    kid: String,
}

fn b64url(data: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(data)
}

fn mint_keys() -> TestKeys {
    let mut rng = rand::thread_rng();
    let private = RsaPrivateKey::new(&mut rng, 2048).expect("rsa key");
    let public = RsaPublicKey::from(&private);
    let pem = private.to_pkcs1_pem(rsa::pkcs8::LineEnding::LF).unwrap();
    let encoding = EncodingKey::from_rsa_pem(pem.as_bytes()).unwrap();
    let kid = "test-kid-1".to_string();
    let n = b64url(&public.n().to_bytes_be());
    let e = b64url(&public.e().to_bytes_be());
    let jwks_json = json!({
        "keys": [{
            "kty": "RSA",
            "kid": kid,
            "alg": "RS256",
            "use": "sig",
            "n": n,
            "e": e
        }]
    })
    .to_string();
    TestKeys {
        encoding,
        jwks_json,
        kid,
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn sign_claims(keys: &TestKeys, claims: Value) -> String {
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(keys.kid.clone());
    // jsonwebtoken wants a serializable claims struct — use Map via Value
    encode(&header, &claims, &keys.encoding).expect("sign")
}

fn oidc_cfg(keys: &TestKeys) -> OidcConfig {
    OidcConfig::new("http://localhost:8080", "graphql-api")
        .with_static_jwks(&keys.jwks_json)
        .engine_roles(&["admin", "customer", "user"])
}

fn headers_from(pairs: &[(&str, &str)]) -> HeaderMap {
    let mut h = HeaderMap::new();
    for (k, v) in pairs {
        h.insert(
            axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
            HeaderValue::from_str(v).unwrap(),
        );
    }
    h
}

// ── F1 / F2 claim map (shipped map_claims_to_session) ───────────────────────

#[test]
fn f1_zitadel_project_roles_session() {
    let claims = json!({
        "iss": "http://localhost:8080",
        "aud": ["graphql-api", "123@graphql"],
        "sub": "user-a-001",
        "exp": 4102444800_i64,
        "urn:zitadel:iam:org:project:roles": {
            "customer": { "280664559058878577": "zitadel.localhost" },
            "admin": { "280664559058878577": "zitadel.localhost" }
        }
    });
    let cfg = ClaimMapConfig {
        engine_roles: vec!["admin".into(), "customer".into(), "user".into()],
        ..Default::default()
    };
    let session = map_claims_to_session(&claims, &cfg).unwrap();
    assert_eq!(session.user_id(), Some("user-a-001"));
    assert_eq!(session.role(), Some("admin"));
    assert_eq!(session.get("x-roles"), Some("admin,customer"));
}

#[test]
fn f2_groups_array_session() {
    let claims = json!({
        "iss": "http://localhost:8080",
        "aud": "graphql-api",
        "sub": "user-b-002",
        "exp": 4102444800_i64,
        "groups": ["customer", "other-unmapped"]
    });
    let cfg = ClaimMapConfig {
        engine_roles: vec!["admin".into(), "customer".into(), "user".into()],
        ..Default::default()
    };
    let session = map_claims_to_session(&claims, &cfg).unwrap();
    assert_eq!(session.user_id(), Some("user-b-002"));
    assert_eq!(session.role(), Some("customer"));
    assert_eq!(session.get("x-roles"), Some("customer"));
}

// ── F3–F5 JWT validation rejects ────────────────────────────────────────────

#[test]
fn f3_alg_none_rejected() {
    let keys = mint_keys();
    let validator = OidcValidator::new(oidc_cfg(&keys));
    // Compact JWT with alg=none (unsigned)
    let header = b64url(br#"{"alg":"none","typ":"JWT"}"#);
    let payload = b64url(
        br#"{"iss":"http://localhost:8080","aud":"graphql-api","sub":"x","exp":4102444800}"#,
    );
    let token = format!("{header}.{payload}.");
    let err = validator.validate_token(&token).unwrap_err();
    assert!(
        matches!(
            err,
            distributed::graphql::ValidationError::AlgNone
                | distributed::graphql::ValidationError::AlgNotAllowed
                | distributed::graphql::ValidationError::Malformed
                | distributed::graphql::ValidationError::Signature
        ),
        "expected alg reject, got {err:?}"
    );
}

#[test]
fn f4_wrong_aud_rejected() {
    let keys = mint_keys();
    let claims = json!({
        "iss": "http://localhost:8080",
        "aud": "other-api",
        "sub": "user-a-001",
        "exp": now() + 3600,
        "iat": now(),
        "groups": ["customer"]
    });
    let token = sign_claims(&keys, claims);
    let err = OidcValidator::new(oidc_cfg(&keys))
        .validate_token(&token)
        .unwrap_err();
    assert!(
        matches!(err, distributed::graphql::ValidationError::Audience)
            || err.to_string().contains("aud"),
        "got {err:?}"
    );
}

#[test]
fn f5_expired_rejected() {
    let keys = mint_keys();
    let claims = json!({
        "iss": "http://localhost:8080",
        "aud": "graphql-api",
        "sub": "user-a-001",
        "exp": 1_000_000_000_i64,
        "iat": 999_999_000_i64,
        "groups": ["customer"]
    });
    let token = sign_claims(&keys, claims);
    let err = OidcValidator::new(oidc_cfg(&keys))
        .validate_token(&token)
        .unwrap_err();
    assert!(
        matches!(err, distributed::graphql::ValidationError::Expired),
        "got {err:?}"
    );
}

// ── F1 success via full validate_and_map + spoof headers ignored ────────────

#[test]
fn f1_valid_jwt_maps_session_spoof_headers_ignored() {
    let keys = mint_keys();
    let claims = json!({
        "iss": "http://localhost:8080",
        "aud": "graphql-api",
        "sub": "user-a-001",
        "exp": now() + 3600,
        "iat": now(),
        "urn:zitadel:iam:org:project:roles": {
            "customer": { "1": "x" },
            "admin": { "1": "x" }
        }
    });
    let token = sign_claims(&keys, claims);
    let mut cfg = IdentityConfig::oidc_bearer(oidc_cfg(&keys));
    let headers = headers_from(&[
        ("authorization", &format!("Bearer {token}")),
        ("x-user-id", "evil"),
        ("x-role", "admin"),
    ]);
    let session = resolve_session_sync(&headers, &cfg).unwrap();
    assert_eq!(session.user_id(), Some("user-a-001"));
    assert_eq!(session.role(), Some("admin"));
    // Client spoof must not replace sub
    assert_ne!(session.user_id(), Some("evil"));
    let _ = &mut cfg;
}

// ── F6 Hybrid missing Bearer → trust gateway headers ────────────────────────

#[test]
fn f6_hybrid_missing_bearer_trusts_proxy_headers() {
    let keys = mint_keys();
    let cfg = IdentityConfig::hybrid(oidc_cfg(&keys));
    let headers = headers_from(&[
        ("x-user-id", "gateway-user-9"),
        ("x-role", "customer"),
    ]);
    let session = resolve_session_sync(&headers, &cfg).unwrap();
    assert_eq!(session.user_id(), Some("gateway-user-9"));
    assert_eq!(session.role(), Some("customer"));
}

// ── F7 Hybrid invalid Bearer → 401, no fallthrough ──────────────────────────

#[test]
fn f7_hybrid_invalid_bearer_no_proxy_fallthrough() {
    let keys = mint_keys();
    let cfg = IdentityConfig::hybrid(oidc_cfg(&keys));
    let header = b64url(br#"{"alg":"none"}"#);
    let payload = b64url(br#"{}"#);
    let bad = format!("{header}.{payload}.");
    let headers = headers_from(&[
        ("authorization", &format!("Bearer {bad}")),
        ("x-user-id", "gateway-user-9"),
        ("x-role", "admin"),
    ]);
    let err = resolve_session_sync(&headers, &cfg).unwrap_err();
    assert_eq!(err, AuthError::Unauthorized);
}

// ── F8 / F9 require_auth ────────────────────────────────────────────────────

#[test]
fn f8_oidc_missing_require_auth_unauthorized() {
    let keys = mint_keys();
    let cfg = IdentityConfig::oidc_bearer(oidc_cfg(&keys)); // require_auth true
    let headers = HeaderMap::new();
    assert_eq!(
        resolve_session_sync(&headers, &cfg).unwrap_err(),
        AuthError::Unauthorized
    );
}

#[test]
fn f9_oidc_missing_require_auth_false_anonymous() {
    let keys = mint_keys();
    let mut oidc = oidc_cfg(&keys);
    oidc.require_auth = false;
    let cfg = IdentityConfig::oidc_bearer(oidc);
    let headers = HeaderMap::new();
    let session = resolve_session_sync(&headers, &cfg).unwrap();
    assert!(session.user_id().is_none());
    assert!(session.role().is_none());
}

// ── F10 TrustedProxy strip ──────────────────────────────────────────────────

#[test]
fn f10_trusted_proxy_strips_client_identity() {
    let cfg = IdentityConfig::trusted_proxy();
    let headers = headers_from(&[
        ("x-user-id", "attacker"),
        ("x-role", "admin"),
        ("x-request-id", "req-1"),
    ]);
    let session = resolve_session_sync(&headers, &cfg).unwrap();
    assert!(session.user_id().is_none(), "x-user-id must be stripped");
    assert!(session.role().is_none(), "x-role must be stripped");
    assert_eq!(session.get("x-request-id"), Some("req-1"));

    // Also exercise strip helper directly
    let stripped = strip_identity_headers(
        &headers,
        &DEFAULT_IDENTITY_STRIP_HEADERS
            .iter()
            .map(|s| (*s).to_string())
            .collect::<Vec<_>>(),
    );
    assert!(stripped.user_id().is_none());
}

#[test]
fn extract_bearer_empty_is_invalid() {
    let headers = headers_from(&[("authorization", "Bearer ")]);
    assert_eq!(extract_bearer(&headers).unwrap_err(), AuthError::Unauthorized);
}

// ── HTTP 401 on real GraphQL router (OidcBearer) ─────────────────────────────

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ReadModel)]
#[table("id_items")]
struct IdItem {
    #[id("id")]
    id: String,
    owner: String,
}

async fn engine_with_identity(identity: IdentityConfig) -> Arc<GraphqlEngine> {
    let pool = SqlitePoolOptions::new()
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::query(
        "CREATE TABLE id_items (id TEXT PRIMARY KEY, owner TEXT NOT NULL);
         INSERT INTO id_items VALUES ('1', 'user-a-001');
         INSERT INTO id_items VALUES ('2', 'other');",
    )
    .execute(&pool)
    .await
    .unwrap();
    let engine = GraphqlEngine::builder(pool)
        .roles(&["customer", "admin", "user"])
        .model::<IdItem>(
            ModelPermissions::new()
                .role(
                    "customer",
                    select()
                        .all_columns()
                        .filter(distributed::graphql::col("owner").eq(
                            distributed::graphql::claim("x-user-id"),
                        )),
                )
                .role("admin", select().all_columns())
                .role("user", select().all_columns()),
        )
        .identity(identity)
        .graphiql(false)
        .build()
        .unwrap();
    Arc::new(engine)
}

#[tokio::test]
async fn http_oidc_missing_bearer_returns_401() {
    let keys = mint_keys();
    let engine = engine_with_identity(IdentityConfig::oidc_bearer(oidc_cfg(&keys))).await;
    let app = graphql_router(engine);
    let res = app
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/graphql")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(
                    r#"{"query":"{ id_items { id } }"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), axum::http::StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn http_oidc_valid_bearer_isolation() {
    let keys = mint_keys();
    let claims = json!({
        "iss": "http://localhost:8080",
        "aud": "graphql-api",
        "sub": "user-a-001",
        "exp": now() + 3600,
        "iat": now(),
        "groups": ["customer"]
    });
    let token = sign_claims(&keys, claims);
    let engine = engine_with_identity(IdentityConfig::oidc_bearer(oidc_cfg(&keys))).await;
    let app = graphql_router(engine);
    let res = app
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/graphql")
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {token}"))
                .header("x-user-id", "evil") // spoof ignored
                .body(axum::body::Body::from(
                    r#"{"query":"{ id_items { id owner } }"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), axum::http::StatusCode::OK);
    let bytes = axum::body::to_bytes(res.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    let rows = v["data"]["id_items"].as_array().expect("data");
    assert_eq!(rows.len(), 1, "isolation: only owner rows: {v}");
    assert_eq!(rows[0]["owner"], "user-a-001");
}

#[tokio::test]
async fn http_hybrid_invalid_bearer_401() {
    let keys = mint_keys();
    let engine = engine_with_identity(IdentityConfig::hybrid(oidc_cfg(&keys))).await;
    let app = graphql_router(engine);
    let res = app
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/graphql")
                .header("content-type", "application/json")
                .header("authorization", "Bearer eyJhbGciOiJub25lIn0.e30.")
                .header("x-role", "admin")
                .body(axum::body::Body::from(
                    r#"{"query":"{ id_items { id } }"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), axum::http::StatusCode::UNAUTHORIZED);
}

#[test]
fn public_scaffold_default_is_oidc_bearer_not_dev() {
    // Drives shipped `public_oidc_identity_from_env_vars` (same function scaffold
    // calls via `public_oidc_identity_from_env`) — D6 never DevHeaders.
    use distributed::graphql::{
        public_oidc_identity_from_env_vars, UNSET_OIDC_AUDIENCE, UNSET_OIDC_ISSUER,
    };

    let unset = public_oidc_identity_from_env_vars(None, None, None, None);
    assert_eq!(unset.mode, IdentityMode::OidcBearer);
    assert_ne!(unset.mode, IdentityMode::DevHeaders);
    assert!(unset.oidc.as_ref().unwrap().require_auth);
    assert_eq!(unset.oidc.as_ref().unwrap().issuer, UNSET_OIDC_ISSUER);
    assert_eq!(unset.oidc.as_ref().unwrap().audience, UNSET_OIDC_AUDIENCE);

    // Ambient headers alone must not authenticate under public default.
    let headers = headers_from(&[("x-user-id", "attacker"), ("x-role", "admin")]);
    assert_eq!(
        resolve_session_sync(&headers, &unset).unwrap_err(),
        AuthError::Unauthorized
    );

    let configured = public_oidc_identity_from_env_vars(
        Some("http://localhost:8080"),
        Some("graphql-api"),
        None,
        None,
    );
    assert_eq!(configured.mode, IdentityMode::OidcBearer);
    assert!(configured.oidc.as_ref().unwrap().require_auth);
    assert_eq!(
        configured.oidc.as_ref().unwrap().issuer,
        "http://localhost:8080"
    );
}

#[test]
fn gateway_secret_wrong_is_401() {
    let mut cfg = IdentityConfig::trusted_proxy();
    cfg.trusted_proxy.gateway_secret_header =
        Some(("x-gateway-secret".into(), "s3cret".into()));
    let headers = headers_from(&[
        ("x-gateway-secret", "wrong"),
        ("x-user-id", "u"),
        ("x-role", "admin"),
    ]);
    assert_eq!(
        resolve_session_sync(&headers, &cfg).unwrap_err(),
        AuthError::Unauthorized
    );
}
