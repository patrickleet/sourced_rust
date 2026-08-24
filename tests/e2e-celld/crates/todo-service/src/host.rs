//! Todo cell wait-path: shard + GraphQL payload. Outbox drain lives in
//! [`distributed::cell_host`].

use distributed::cell_host::CelldRoute;
use distributed::microsvc::Session;
use serde_json::{json, Value};

const TODO_CELL_COMMANDS: &[&str] = &[
    "todo.create",
    "todo.rename",
    "todo.complete",
    "todo.reopen",
    "todo.archive",
    "todo.force_archive",
    "todo.purge",
];

/// Route every Todo aggregate transition to the same cell shard.
pub fn celld_route() -> CelldRoute {
    CelldRoute::new(TODO_CELL_COMMANDS, "todo", todo_shard, graphql_todo_payload)
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
    let status = remote
        .get("status")
        .cloned()
        .unwrap_or_else(|| match command {
            "todo.complete" => json!("completed"),
            "todo.archive" | "todo.force_archive" => json!("archived"),
            _ => json!("open"),
        });
    match command {
        "todo.create" => json!({
            "todo_id": id,
            "owner_id": remote
                .get("owner_id")
                .cloned()
                .or_else(|| session.user_id().map(|id| json!(id)))
                .unwrap_or(json!("")),
            "title": remote.get("title").or_else(|| input.get("title")).cloned().unwrap_or(json!("")),
            "status": status,
        }),
        "todo.rename" => json!({
            "todo_id": id,
            "title": remote.get("title").or_else(|| input.get("title")).cloned().unwrap_or(json!("")),
            "status": status,
        }),
        "todo.complete" | "todo.reopen" | "todo.archive" => {
            json!({ "todo_id": id, "status": status })
        }
        "todo.force_archive" => json!({
            "todo_id": id,
            "owner_id": remote.get("owner_id").cloned().unwrap_or(json!("")),
            "status": status,
            "archived_by": remote
                .get("archived_by")
                .cloned()
                .or_else(|| session.user_id().map(|id| json!(id)))
                .unwrap_or(json!("")),
        }),
        "todo.purge" => json!({
            "todo_id": id,
            "purged": remote.get("purged").cloned().unwrap_or(json!(true)),
        }),
        _ => remote.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn celld_route_keeps_every_todo_transition_on_one_shard() {
        assert_eq!(celld_route().commands, TODO_CELL_COMMANDS);
    }
}
