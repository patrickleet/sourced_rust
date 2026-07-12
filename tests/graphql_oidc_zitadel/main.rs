//! Env-gated live Zitadel e2e (D11/D12).
//!
//! - Without `ZITADEL_E2E=1`: suite runs and soft-skips (offline CI / unit matrix).
//! - With `ZITADEL_E2E=1` + issuer ready + bootstrap env: **hard-fail** on mint/validate
//!   errors (GitHub Actions live job).
//! Token mint: JWT-bearer grant only (machine-user keys).

#![cfg(all(feature = "graphql", feature = "sqlite"))]

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use distributed::graphql::{OidcConfig, OidcValidator};
use serde_json::{json, Value};

fn e2e_enabled() -> bool {
    matches!(
        std::env::var("ZITADEL_E2E")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes"
    )
}

fn issuer() -> Option<String> {
    std::env::var("OIDC_ISSUER")
        .ok()
        .filter(|s| !s.is_empty())
}

async fn issuer_ready(iss: &str) -> bool {
    let base = iss.trim_end_matches('/');
    let url = format!("{base}/debug/ready");
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .ok();
    let Some(client) = client else {
        return false;
    };
    client
        .get(&url)
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

/// Offline path: always runs in CI without Zitadel (D12).
#[tokio::test]
async fn zitadel_e2e_skips_when_not_gated() {
    if e2e_enabled() {
        // Live path covered by `zitadel_e2e_live_jwt_bearer_mint`.
        return;
    }
    eprintln!("ZITADEL_E2E not set — skip live path (D12); suite binary still executes in CI");
}

/// Live path: mint real access token via JWT-bearer and validate with shipped OIDC stack.
#[tokio::test]
async fn zitadel_e2e_live_jwt_bearer_mint() {
    if !e2e_enabled() {
        eprintln!("ZITADEL_E2E not set — skip live mint");
        return;
    }

    let iss = issuer().expect("ZITADEL_E2E=1 requires OIDC_ISSUER");
    assert!(
        issuer_ready(&iss).await,
        "ZITADEL_E2E=1 but issuer not ready at {iss}"
    );

    let key_path = std::env::var("GRAPHQL_E2E_CUSTOMER_KEY")
        .expect("ZITADEL_E2E=1 requires GRAPHQL_E2E_CUSTOMER_KEY (path to machine key JSON)");
    let uid = std::env::var("GRAPHQL_E2E_CUSTOMER_USER_ID")
        .expect("ZITADEL_E2E=1 requires GRAPHQL_E2E_CUSTOMER_USER_ID");
    let audience = std::env::var("OIDC_AUDIENCE")
        .or_else(|_| std::env::var("OIDC_CLIENT_ID"))
        .expect("ZITADEL_E2E=1 requires OIDC_AUDIENCE or OIDC_CLIENT_ID");

    let token = mint_jwt_bearer(&iss, &key_path, &uid)
        .await
        .unwrap_or_else(|e| panic!("JWT-bearer mint failed: {e}"));
    assert!(!token.is_empty(), "access token empty");
    eprintln!("minted access token (len={})", token.len());

    // Validate via shipped OIDC path (discovery + JWKS, not a reimplemented oracle).
    let oidc = OidcConfig::new(&iss, &audience);
    let validator = OidcValidator::new(oidc);
    let session = validator
        .validate_and_map_async(&token)
        .await
        .unwrap_or_else(|e| panic!("shipped OIDC validate failed: {e}"));
    assert_eq!(
        session.user_id(),
        Some(uid.as_str()),
        "Session x-user-id must be JWT sub / machine user id"
    );
    eprintln!("live Zitadel → Session user_id={:?}", session.user_id());
}

/// Mint access token: grant_type=urn:ietf:params:oauth:grant-type:jwt-bearer (D11).
async fn mint_jwt_bearer(issuer: &str, key_path: &str, user_id: &str) -> Result<String, String> {
    let path = PathBuf::from(key_path);
    let raw = std::fs::read_to_string(&path).map_err(|e| format!("read key: {e}"))?;
    let key_json: Value = serde_json::from_str(&raw).map_err(|e| format!("parse key: {e}"))?;
    let key_id = key_json
        .get("keyId")
        .or_else(|| key_json.get("key_id"))
        .and_then(|v| v.as_str())
        .ok_or("keyId missing")?;
    let private_pem = key_json
        .get("key")
        .and_then(|v| v.as_str())
        .ok_or("key PEM missing")?;

    let iss = issuer.trim_end_matches('/');
    let token_url = format!("{iss}/oauth/v2/token");
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let assertion_claims = json!({
        "iss": user_id,
        "sub": user_id,
        "aud": [token_url.clone(), iss],
        "iat": now,
        "exp": now + 60,
    });

    let encoding = jsonwebtoken::EncodingKey::from_rsa_pem(private_pem.as_bytes())
        .map_err(|e| format!("pem: {e}"))?;
    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
    header.kid = Some(key_id.to_string());
    let assertion = jsonwebtoken::encode(&header, &assertion_claims, &encoding)
        .map_err(|e| format!("sign: {e}"))?;

    let client = reqwest::Client::new();
    let body = format!(
        "grant_type={}&scope={}&assertion={}",
        urlencoding("urn:ietf:params:oauth:grant-type:jwt-bearer"),
        urlencoding("openid profile urn:zitadel:iam:org:project:roles"),
        urlencoding(&assertion),
    );
    let resp = client
        .post(&token_url)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .map_err(|e| format!("token request: {e}"))?;
    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("token endpoint error: {body}"));
    }
    let body: Value = resp.json().await.map_err(|e| format!("json: {e}"))?;
    body.get("access_token")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "no access_token".into())
}

fn urlencoding(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
