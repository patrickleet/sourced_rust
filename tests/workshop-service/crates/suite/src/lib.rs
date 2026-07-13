//! Shared helpers for the workshop behavioral suite.
//!
//! The suite speaks **HTTP only** (commands + GraphQL) against `WORKSHOP_BASE_URL`.
//! Topology (monolith vs multi-service) is chosen by how the process is started —
//! assertions never branch on process layout.

use std::time::Duration;

use serde_json::Value;

/// Base URL for the process under test (gateway or monolith).
pub fn base_url() -> String {
    std::env::var("WORKSHOP_BASE_URL").unwrap_or_else(|_| "http://127.0.0.1:8791".into())
}

pub async fn wait_ready(base: &str, timeout: Duration) -> bool {
    let client = reqwest::Client::new();
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        // GraphQL endpoint exists even if unauthorized for empty body
        if let Ok(resp) = client
            .post(format!("{base}/graphql"))
            .header("content-type", "application/json")
            .json(&serde_json::json!({"query":"{ __typename }"}))
            .send()
            .await
        {
            if resp.status().as_u16() < 500 {
                return true;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

pub async fn post_command(
    base: &str,
    command: &str,
    body: Value,
    user_id: &str,
    role: &str,
) -> Result<Value, String> {
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{base}/{command}"))
        .header("content-type", "application/json")
        .header("x-user-id", user_id)
        .header("x-role", role)
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = resp.status();
    let v: Value = resp.json().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("command {command} HTTP {status}: {v}"));
    }
    Ok(v)
}

pub async fn graphql(
    base: &str,
    query: &str,
    user_id: &str,
    role: &str,
) -> Result<Value, String> {
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{base}/graphql"))
        .header("content-type", "application/json")
        .header("x-user-id", user_id)
        .header("x-role", role)
        .json(&serde_json::json!({ "query": query }))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = resp.status();
    let v: Value = resp.json().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("graphql HTTP {status}: {v}"));
    }
    Ok(v)
}

/// Shared case IDs (same for monolith and multi).
pub mod cases {
    pub const LIST_PRODUCT: &str = "W1_list_product";
    pub const PLACE_ORDER: &str = "W2_place_order";
    pub const GRAPHQL_PRODUCTS: &str = "W3_graphql_products";
    pub const GRAPHQL_ORDER_ISOLATION: &str = "W4_graphql_order_isolation";
}
