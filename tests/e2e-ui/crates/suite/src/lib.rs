//! Shared HTTP helpers for the todo behavioral suite.

use std::time::Duration;

use serde_json::Value;

pub fn base_url() -> String {
    std::env::var("E2E_BASE_URL").unwrap_or_else(|_| "http://127.0.0.1:8791".into())
}

pub async fn wait_ready(base: &str, timeout: Duration) -> bool {
    let client = reqwest::Client::new();
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        let req = client
            .post(format!("{base}/graphql"))
            .header("content-type", "application/json")
            .header("x-user-id", "probe")
            .header("x-role", "admin")
            .json(&serde_json::json!({"query":"{ __typename }"}));
        if let Ok(resp) = req.send().await {
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

pub async fn post_command_raw(
    base: &str,
    command: &str,
    body: Value,
    user_id: Option<&str>,
    role: Option<&str>,
) -> Result<(u16, Value), String> {
    let client = reqwest::Client::new();
    let mut req = client
        .post(format!("{base}/{command}"))
        .header("content-type", "application/json")
        .json(&body);
    if let Some(u) = user_id {
        req = req.header("x-user-id", u);
    }
    if let Some(r) = role {
        req = req.header("x-role", r);
    }
    let resp = req.send().await.map_err(|e| e.to_string())?;
    let status = resp.status().as_u16();
    let v: Value = resp.json().await.unwrap_or(serde_json::json!({}));
    Ok((status, v))
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

pub mod cases {
    pub const CREATE: &str = "T1_create_todo";
    pub const OWNER_ISOLATION: &str = "T2_owner_isolation";
    pub const COMPLETE: &str = "T3_complete_todo";
    pub const NOT_OWNER: &str = "T4_not_owner_rejected";
    pub const UNAUTH: &str = "T5_unauthenticated_rejected";
    pub const LIFECYCLE: &str = "T6_lifecycle_rename_archive";
}
