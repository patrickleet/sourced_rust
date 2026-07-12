//! Okta live e2e — E1–E8. Gate: `OKTA_E2E=1`.
//! Mint: client_credentials ([[specs/query-layer/oidc-okta]]).
//! Without secrets when gated → **hard-fail** (not soft-skip).

#![cfg(all(feature = "graphql", feature = "sqlite"))]

#[path = "../graphql_oidc_common/mod.rs"]
mod common;

use distributed::graphql::OidcConfig;
use serde_json::Value;

fn e2e_enabled() -> bool {
    common::gate_enabled("OKTA_E2E")
}

#[tokio::test]
async fn e0_skips_when_not_gated() {
    if e2e_enabled() {
        return;
    }
    eprintln!("OKTA_E2E not set — skip live E1–E8 (offline CI)");
}

#[tokio::test]
async fn e1_through_e8_live_or_hard_fail_without_secrets() {
    if !e2e_enabled() {
        eprintln!("OKTA_E2E not set — skip");
        return;
    }

    let iss = std::env::var("OIDC_ISSUER").unwrap_or_default();
    let audience = std::env::var("OIDC_AUDIENCE").unwrap_or_default();
    let token_url = std::env::var("OKTA_TOKEN_URL").unwrap_or_default();
    let c_id = std::env::var("OKTA_E2E_CUSTOMER_CLIENT_ID").unwrap_or_default();
    let c_sec = std::env::var("OKTA_E2E_CUSTOMER_CLIENT_SECRET").unwrap_or_default();
    let a_id = std::env::var("OKTA_E2E_ADMIN_CLIENT_ID").unwrap_or_default();
    let a_sec = std::env::var("OKTA_E2E_ADMIN_CLIENT_SECRET").unwrap_or_default();
    let scope = std::env::var("OKTA_E2E_SCOPE").unwrap_or_default();

    let missing: Vec<&str> = [
        ("OIDC_ISSUER", iss.as_str()),
        ("OIDC_AUDIENCE", audience.as_str()),
        ("OKTA_TOKEN_URL", token_url.as_str()),
        ("OKTA_E2E_CUSTOMER_CLIENT_ID", c_id.as_str()),
        ("OKTA_E2E_CUSTOMER_CLIENT_SECRET", c_sec.as_str()),
        ("OKTA_E2E_ADMIN_CLIENT_ID", a_id.as_str()),
        ("OKTA_E2E_ADMIN_CLIENT_SECRET", a_sec.as_str()),
        ("OKTA_E2E_SCOPE", scope.as_str()),
    ]
    .into_iter()
    .filter(|(_, v)| v.is_empty())
    .map(|(k, _)| k)
    .collect();

    if !missing.is_empty() {
        panic!(
            "OKTA_E2E=1 but required env missing: {}. \
             Set Okta API Services credentials (see specs/query-layer/oidc-okta). \
             Do not soft-skip when gated.",
            missing.join(", ")
        );
    }

    assert!(
        common::discovery_ready(&iss).await,
        "Okta discovery not ready at {iss}"
    );

    let mut oidc = OidcConfig::new(&iss, &audience);
    oidc.claim_map.role_claims = vec!["groups".into(), "roles".into(), "graphql_roles".into()];

    common::run_e1_through_e8(
        oidc,
        async {
            mint_client_credentials(&token_url, &c_id, &c_sec, &scope).await
        },
        async {
            mint_client_credentials(&token_url, &a_id, &a_sec, &scope).await
        },
    )
    .await;
}

async fn mint_client_credentials(
    token_url: &str,
    client_id: &str,
    client_secret: &str,
    scope: &str,
) -> Result<(String, String), String> {
    let body = format!(
        "grant_type=client_credentials&client_id={}&client_secret={}&scope={}",
        urlencoding(client_id),
        urlencoding(client_secret),
        urlencoding(scope)
    );
    let resp = reqwest::Client::new()
        .post(token_url)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .map_err(|e| format!("token: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!(
            "token HTTP {}: {}",
            resp.status(),
            resp.text().await.unwrap_or_default()
        ));
    }
    let v: Value = resp.json().await.map_err(|e| e.to_string())?;
    let token = v
        .get("access_token")
        .and_then(|t| t.as_str())
        .ok_or("no access_token")?
        .to_string();
    let sub = decode_sub(&token).ok_or("no sub")?;
    Ok((token, sub))
}

fn decode_sub(token: &str) -> Option<String> {
    let payload = token.split('.').nth(1)?;
    let mut s = payload.replace('-', "+").replace('_', "/");
    while s.len() % 4 != 0 {
        s.push('=');
    }
    let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, s.as_bytes())
        .ok()?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    v.get("sub")?.as_str().map(|s| s.to_string())
}

fn urlencoding(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
