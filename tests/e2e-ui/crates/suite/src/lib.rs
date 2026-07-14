//! Shared helpers for the e2e-ui suite — GraphQL-only public API.

use std::time::Duration;

use serde_json::{json, Value};

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
            .json(&json!({"query":"{ __typename }"}));
        if let Ok(resp) = req.send().await {
            if resp.status().as_u16() < 500 {
                return true;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
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
        .json(&json!({ "query": query }))
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

/// GraphQL document without identity headers (unauthenticated probe).
pub async fn graphql_raw(base: &str, query: &str) -> Result<(u16, Value), String> {
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{base}/graphql"))
        .header("content-type", "application/json")
        .json(&json!({ "query": query }))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = resp.status().as_u16();
    let v: Value = resp.json().await.unwrap_or(json!({}));
    Ok((status, v))
}

fn gql_errors(v: &Value) -> Option<String> {
    let errs = v.get("errors")?.as_array()?;
    if errs.is_empty() {
        return None;
    }
    Some(errs.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("; "))
}

/// Run a mutation and return `data.<field>` or Err with GraphQL errors.
pub async fn mutate(
    base: &str,
    field: &str,
    document: &str,
    user_id: &str,
    role: &str,
) -> Result<Value, String> {
    let v = graphql(base, document, user_id, role).await?;
    if let Some(msg) = gql_errors(&v) {
        return Err(format!("{field} errors: {msg}"));
    }
    v["data"][field]
        .as_object()
        .cloned()
        .map(Value::Object)
        .ok_or_else(|| format!("{field} missing data: {v}"))
}

pub async fn todos_create(
    base: &str,
    todo_id: &str,
    title: &str,
    user_id: &str,
    role: &str,
) -> Result<Value, String> {
    let doc = format!(
        r#"mutation {{
          todos_create(input: {{ todo_id: "{todo_id}", title: "{title}" }}) {{
            todo_id owner_id title status
          }}
        }}"#
    );
    mutate(base, "todos_create", &doc, user_id, role).await
}

pub async fn todos_complete(
    base: &str,
    todo_id: &str,
    user_id: &str,
    role: &str,
) -> Result<Value, String> {
    let doc = format!(
        r#"mutation {{
          todos_complete(input: {{ todo_id: "{todo_id}" }}) {{
            todo_id status
          }}
        }}"#
    );
    mutate(base, "todos_complete", &doc, user_id, role).await
}

pub async fn todos_archive(
    base: &str,
    todo_id: &str,
    user_id: &str,
    role: &str,
) -> Result<Value, String> {
    let doc = format!(
        r#"mutation {{
          todos_archive(input: {{ todo_id: "{todo_id}" }}) {{
            todo_id status
          }}
        }}"#
    );
    mutate(base, "todos_archive", &doc, user_id, role).await
}

/// Admin-only mutation (not present on the user role schema).
pub async fn todos_force_archive(
    base: &str,
    todo_id: &str,
    user_id: &str,
    role: &str,
) -> Result<Value, String> {
    let doc = format!(
        r#"mutation {{
          todos_force_archive(input: {{ todo_id: "{todo_id}" }}) {{
            todo_id owner_id status archived_by
          }}
        }}"#
    );
    mutate(base, "todos_force_archive", &doc, user_id, role).await
}

pub async fn todos_rename(
    base: &str,
    todo_id: &str,
    title: &str,
    user_id: &str,
    role: &str,
) -> Result<Value, String> {
    let doc = format!(
        r#"mutation {{
          todos_rename(input: {{ todo_id: "{todo_id}", title: "{title}" }}) {{
            todo_id title status
          }}
        }}"#
    );
    mutate(base, "todos_rename", &doc, user_id, role).await
}

/// Assert HTTP command routes are not mounted (GraphQL-only surface).
pub async fn assert_http_commands_disabled(base: &str) -> Result<(), String> {
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{base}/todo.create"))
        .header("content-type", "application/json")
        .header("x-user-id", "alice")
        .header("x-role", "user")
        .json(&json!({ "todo_id": "t-should-404", "title": "nope" }))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = resp.status().as_u16();
    // No route → 404 (or 405 if something else matches).
    if status != 404 && status != 405 {
        return Err(format!(
            "expected HTTP command route disabled (404/405), got {status}"
        ));
    }
    Ok(())
}

pub mod cases {
    pub const CREATE: &str = "T1_create_todo";
    pub const OWNER_ISOLATION: &str = "T2_owner_isolation";
    pub const ADMIN_SEES_ALL: &str = "T2b_admin_sees_all_owners";
    pub const ADMIN_FORCE_ARCHIVE: &str = "T2c_admin_force_archive";
    pub const COMPLETE: &str = "T3_complete_todo";
    pub const NOT_OWNER: &str = "T4_not_owner_rejected";
    pub const UNAUTH: &str = "T5_unauthenticated_rejected";
    pub const LIFECYCLE: &str = "T6_lifecycle_rename_archive";
    pub const HTTP_OFF: &str = "T0_http_commands_disabled";
}
