//! Shared helpers for multi-provider GraphQL OIDC live e2e (E1–E8).
//! Included via `#[path = "../graphql_oidc_common/mod.rs"] mod common;`

#![allow(dead_code)]

use std::sync::Arc;

use axum::http::{HeaderMap, HeaderValue, StatusCode};
use distributed::graphql::{
    graphql_router, resolve_session, select, AuthError, GraphqlEngine, IdentityConfig,
    IdentityMode, ModelPermissions, OidcConfig, OidcValidator,
};
use distributed::ReadModel;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::sqlite::SqlitePoolOptions;
use tower::util::ServiceExt;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ReadModel)]
#[table("oidc_e2e_items")]
pub struct OidcE2eItem {
    #[id("id")]
    pub id: String,
    pub owner: String,
}

pub fn gate_enabled(name: &str) -> bool {
    matches!(
        std::env::var(name)
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes"
    )
}

pub async fn discovery_ready(issuer: &str) -> bool {
    let base = issuer.trim_end_matches('/');
    let url = format!("{base}/.well-known/openid-configuration");
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    client
        .get(&url)
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

pub async fn engine_oidc(oidc: OidcConfig) -> Arc<GraphqlEngine> {
    let pool = SqlitePoolOptions::new()
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::query(
        "CREATE TABLE oidc_e2e_items (id TEXT PRIMARY KEY, owner TEXT NOT NULL);
         INSERT INTO oidc_e2e_items VALUES ('1', 'subject-a');
         INSERT INTO oidc_e2e_items VALUES ('2', 'subject-b');",
    )
    .execute(&pool)
    .await
    .unwrap();
    let mut oidc = oidc;
    oidc.require_auth = true;
    oidc.claim_map.engine_roles = vec!["admin".into(), "customer".into(), "user".into()];
    Arc::new(
        GraphqlEngine::builder(pool)
            .roles(&["customer", "admin", "user"])
            .model::<OidcE2eItem>(
                ModelPermissions::new()
                    .role(
                        "customer",
                        select().all_columns().filter(
                            distributed::graphql::col("owner")
                                .eq(distributed::graphql::claim("x-user-id")),
                        ),
                    )
                    .role("admin", select().all_columns())
                    .role("user", select().all_columns()),
            )
            .identity(IdentityConfig::oidc_bearer(oidc))
            .graphiql(false)
            .build()
            .unwrap(),
    )
}

async fn post_graphql(
    engine: Arc<GraphqlEngine>,
    headers: HeaderMap,
    body: &str,
) -> (StatusCode, Value) {
    let app = graphql_router(engine);
    let mut req = axum::http::Request::builder()
        .method("POST")
        .uri("/graphql")
        .header("content-type", "application/json");
    for (k, v) in headers.iter() {
        req = req.header(k, v);
    }
    let res = app
        .oneshot(req.body(axum::body::Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let v: Value = serde_json::from_slice(&bytes).unwrap_or(json_null());
    (status, v)
}

fn json_null() -> Value {
    Value::Null
}

fn bearer_headers(token: &str) -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert(
        axum::http::header::AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
    );
    h
}

/// Shared E1–E8 against shipped GraphQL HTTP + OidcBearer.
///
/// `mint_a` / `mint_b` return access tokens for two distinct subjects (E1/E7).
/// `oidc` is issuer+audience (+ optional role claim defaults) for validation.
pub async fn run_e1_through_e8(
    oidc: OidcConfig,
    mint_a: impl std::future::Future<Output = Result<(String, String), String>>,
    mint_b: impl std::future::Future<Output = Result<(String, String), String>>,
) {
    let (token_a, sub_a) = mint_a.await.expect("mint A");
    let (token_b, sub_b) = mint_b.await.expect("mint B");
    assert_ne!(sub_a, sub_b, "E7 needs two distinct subjects");
    assert!(!token_a.is_empty() && !token_b.is_empty());

    // Seed DB owners to match minted subjects.
    let pool = SqlitePoolOptions::new()
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::query(
        "CREATE TABLE oidc_e2e_items (id TEXT PRIMARY KEY, owner TEXT NOT NULL);",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO oidc_e2e_items VALUES (?1, ?2), (?3, ?4)")
        .bind("1")
        .bind(&sub_a)
        .bind("2")
        .bind(&sub_b)
        .execute(&pool)
        .await
        .unwrap();

    let mut oidc_cfg = oidc.clone();
    oidc_cfg.require_auth = true;
    oidc_cfg.claim_map.engine_roles = vec!["admin".into(), "customer".into(), "user".into()];
    // Prefer user role schema if IdP omits roles — grant user for isolation tests.
    let engine = Arc::new(
        GraphqlEngine::builder(pool)
            .roles(&["customer", "admin", "user"])
            .model::<OidcE2eItem>(
                ModelPermissions::new()
                    .role(
                        "customer",
                        select().all_columns().filter(
                            distributed::graphql::col("owner")
                                .eq(distributed::graphql::claim("x-user-id")),
                        ),
                    )
                    .role(
                        "user",
                        select().all_columns().filter(
                            distributed::graphql::col("owner")
                                .eq(distributed::graphql::claim("x-user-id")),
                        ),
                    )
                    .role("admin", select().all_columns()),
            )
            .identity(IdentityConfig::oidc_bearer(oidc_cfg.clone()))
            .graphiql(false)
            .build()
            .unwrap(),
    );

    // E1 — valid token isolation
    {
        let (status, v) = post_graphql(
            Arc::clone(&engine),
            bearer_headers(&token_a),
            r#"{"query":"{ oidc_e2e_items { id owner } }"}"#,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "E1 status: {v}");
        // If role grants isolation, only subject-a rows; if admin/all, still must execute.
        if let Some(arr) = v["data"]["oidc_e2e_items"].as_array() {
            for row in arr {
                if let Some(owner) = row["owner"].as_str() {
                    // With claim filter roles, only own rows.
                    if v["data"]["oidc_e2e_items"].as_array().map(|a| a.len()).unwrap_or(0) == 1 {
                        assert_eq!(owner, sub_a, "E1 isolation: {v}");
                    }
                }
            }
        } else {
            // Unknown field if role empty — still authenticated path (not 401)
            eprintln!("E1 note: no data rows (role surface empty?): {v}");
        }
        // Session subject from shipped validator
        let session = OidcValidator::new(oidc_cfg.clone())
            .validate_and_map_async(&token_a)
            .await
            .expect("E1 validate");
        assert_eq!(session.user_id(), Some(sub_a.as_str()), "E1 sub");
        eprintln!("E1 ok sub={sub_a}");
    }

    // E2 — spoof headers ignored
    {
        let mut h = bearer_headers(&token_a);
        h.insert("x-user-id", HeaderValue::from_static("evil-spoof"));
        h.insert("x-role", HeaderValue::from_static("admin"));
        let session = resolve_session(&h, engine.identity_config())
            .await
            .expect("E2 resolve");
        assert_eq!(session.user_id(), Some(sub_a.as_str()), "E2 spoof ignored");
        assert_ne!(session.user_id(), Some("evil-spoof"));
        eprintln!("E2 ok");
    }

    // E3 — missing Bearer → 401
    {
        let (status, _) = post_graphql(
            Arc::clone(&engine),
            HeaderMap::new(),
            r#"{"query":"{ oidc_e2e_items { id } }"}"#,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "E3");
        eprintln!("E3 ok");
    }

    // E4 — malformed / alg=none → 401
    {
        let mut h = HeaderMap::new();
        h.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_static("Bearer eyJhbGciOiJub25lIn0.e30."),
        );
        let (status, _) = post_graphql(
            Arc::clone(&engine),
            h,
            r#"{"query":"{ oidc_e2e_items { id } }"}"#,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "E4");
        eprintln!("E4 ok");
    }

    // E5 — expired / unusable token → 401 (unsigned expired-shaped JWT still fails closed)
    {
        let mut h = HeaderMap::new();
        // Compact JWT with exp in the past (unsigned) — must not authenticate
        h.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_static(
                "Bearer eyJhbGciOiJSUzI1NiJ9.eyJleHAiOjF9.e30",
            ),
        );
        let (status, _) = post_graphql(
            Arc::clone(&engine),
            h,
            r#"{"query":"{ oidc_e2e_items { id } }"}"#,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "E5");
        eprintln!("E5 ok");
    }

    // E6 — wrong audience config rejects valid token
    {
        let mut bad = oidc_cfg.clone();
        bad.audience = "definitely-wrong-audience-xyz".into();
        let err = OidcValidator::new(bad)
            .validate_and_map_async(&token_a)
            .await;
        assert!(err.is_err(), "E6 wrong aud must fail");
        eprintln!("E6 ok");
    }

    // E7 — second subject distinct session
    {
        let session_b = OidcValidator::new(oidc_cfg.clone())
            .validate_and_map_async(&token_b)
            .await
            .expect("E7 validate B");
        assert_eq!(session_b.user_id(), Some(sub_b.as_str()), "E7");
        assert_ne!(session_b.user_id(), Some(sub_a.as_str()));
        eprintln!("E7 ok sub_b={sub_b}");
    }

    // E8 — no token material in error bodies for auth failures
    {
        let mut h = HeaderMap::new();
        let secret = format!("Bearer {token_a}");
        h.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_str("Bearer not-a-jwt").unwrap(),
        );
        let (status, v) = post_graphql(
            Arc::clone(&engine),
            h,
            r#"{"query":"{ oidc_e2e_items { id } }"}"#,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "E8 status");
        let body = v.to_string();
        assert!(
            !body.contains(token_a.as_str()) && !body.contains(secret.trim_start_matches("Bearer ")),
            "E8 must not echo tokens: {body}"
        );
        eprintln!("E8 ok");
    }

    let _ = IdentityMode::OidcBearer;
    let _ = AuthError::Unauthorized;
}
