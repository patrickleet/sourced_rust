//! Command: `chat.post` — author is always the authenticated session user.

use chat_domain::{ChatMessage, ChatMessagePosted};
use distributed::microsvc::{Context, HandlerError};
use distributed::OutboxMessage;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::deps::ChatDeps;
use crate::handlers::util::{rejected, require_user, session_has_user};

pub const COMMAND: &str = "chat.post";

#[derive(Debug, Deserialize, distributed::GraphqlInput)]
pub struct ChatPostInput {
    pub message_id: String,
    pub body: String,
    #[serde(default = "default_room")]
    pub room_id: String,
}

fn default_room() -> String {
    "lobby".into()
}

#[derive(Debug, Serialize, distributed::GraphqlOutput)]
pub struct ChatPostPayload {
    pub message_id: String,
    pub room_id: String,
    pub author_id: String,
    pub body: String,
    pub created_at: String,
}

pub fn guard<R, L, S>(ctx: &Context<ChatDeps<R, L, S>>) -> bool
where
    R: crate::bounds::EventStore,
    L: crate::bounds::Locks,
    S: Send + Sync + 'static,
{
    ctx.has_fields(&["message_id", "body"]) && session_has_user(ctx.session())
}

pub async fn handle<R, L, S>(
    ctx: &Context<'_, ChatDeps<R, L, S>>,
) -> Result<Value, HandlerError>
where
    R: crate::bounds::EventStore,
    L: crate::bounds::Locks,
    S: Send + Sync + 'static,
{
    let author = require_user(ctx.session())?;
    let input = ctx.input::<ChatPostInput>()?;

    if ctx.repo().get(&input.message_id).await?.is_some() {
        return Err(HandlerError::Rejected(format!(
            "message {} already exists",
            input.message_id
        )));
    }

    let body = input.body.trim().to_string();
    if body.is_empty() {
        return Err(rejected("empty body"));
    }
    let created_at = chrono_lite_now();
    let mut msg = ChatMessage::default();
    msg.post(
        input.message_id.clone(),
        input.room_id.clone(),
        author,
        body,
        created_at,
    )
    .map_err(rejected)?;

    let fact = ChatMessagePosted::from_message(&msg);
    let outbox = OutboxMessage::encode(
        format!(
            "{}:chat_message.posted:{}",
            msg.message_id,
            msg.entity.version()
        ),
        "chat_message.posted",
        &fact,
    )
    .map_err(|e| HandlerError::Other(Box::new(e)))?;

    ctx.repo().outbox(outbox).commit(&mut msg).await?;

    Ok(json!({
        "message_id": fact.message_id,
        "room_id": fact.room_id,
        "author_id": fact.author_id,
        "body": fact.body,
        "created_at": fact.created_at,
    }))
}

fn chrono_lite_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let d = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    // Unix epoch seconds as a sortable string (no chrono dep in the fixture).
    format!("{}", d.as_millis())
}
