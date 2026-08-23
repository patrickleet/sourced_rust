//! Chat cell wait-path: shard + GraphQL payload. Outbox drain lives in
//! [`distributed::cell_host`].

use distributed::cell_host::CelldRoute;
use distributed::microsvc::Session;
use serde_json::{json, Value};

/// `POST {CELLD_URL}/chat/{message_id}/chat.post`.
pub fn celld_route() -> CelldRoute {
    CelldRoute::new(&["chat.post"], "chat", chat_shard, graphql_chat_payload)
}

fn chat_shard(input: &Value) -> Option<String> {
    input
        .get("message_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn graphql_chat_payload(_command: &str, input: &Value, remote: &Value, session: &Session) -> Value {
    json!({
        "message_id": remote.get("message_id").or_else(|| input.get("message_id")).cloned().unwrap_or(json!("")),
        "room_id": remote.get("room_id").or_else(|| input.get("room_id")).cloned().unwrap_or(json!("lobby")),
        "author_id": remote
            .get("author_id")
            .cloned()
            .or_else(|| session.user_id().map(|id| json!(id)))
            .unwrap_or(json!("")),
        "body": remote.get("body").or_else(|| input.get("body")).cloned().unwrap_or(json!("")),
        "created_at": remote.get("created_at").or_else(|| input.get("created_at")).cloned().unwrap_or(json!("")),
    })
}
