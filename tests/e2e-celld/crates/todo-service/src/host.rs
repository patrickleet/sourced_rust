//! Todo cell wait-path: shard + GraphQL payload. Outbox drain lives in
//! [`distributed::cell_host`].

use distributed::cell_host::CelldRoute;
use distributed::microsvc::Session;
use serde_json::{json, Value};

/// `POST {CELLD_URL}/todo/{todo_id}/{todo.create|todo.complete}`.
pub fn celld_route() -> CelldRoute {
    CelldRoute::new(
        &["todo.create", "todo.complete"],
        "todo",
        todo_shard,
        graphql_todo_payload,
    )
}

fn todo_shard(input: &Value) -> Option<String> {
    input
        .get("todo_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn graphql_todo_payload(command: &str, input: &Value, remote: &Value, session: &Session) -> Value {
    let id = remote
        .get("todo_id")
        .or_else(|| remote.get("id"))
        .or_else(|| input.get("todo_id"))
        .cloned()
        .unwrap_or(json!(""));
    let status = remote.get("status").cloned().unwrap_or_else(|| {
        if command == "todo.complete" {
            json!("completed")
        } else {
            json!("open")
        }
    });
    if command == "todo.complete" {
        json!({ "todo_id": id, "status": status })
    } else {
        json!({
            "todo_id": id,
            "owner_id": remote
                .get("owner_id")
                .cloned()
                .or_else(|| session.user_id().map(|id| json!(id)))
                .unwrap_or(json!("")),
            "title": remote.get("title").or_else(|| input.get("title")).cloned().unwrap_or(json!("")),
            "status": status,
        })
    }
}
