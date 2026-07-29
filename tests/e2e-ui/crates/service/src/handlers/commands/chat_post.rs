//! Command: `chat.post` — author is always the authenticated session user.

use chat_domain::{ChatMessage, ChatMessageState};
use distributed::graphql::{Causal, PreparedCommand};
use distributed::microsvc::{CausalCommandContext, HandlerError};
use serde::{Deserialize, Serialize};

use crate::handlers::util::rejected;

pub const COMMAND: &str = "chat.post";

#[derive(Debug, Deserialize, distributed::GraphqlInput)]
pub struct ChatPostInput {
    pub message_id: String,
    pub body: String,
    pub room_id: String,
    /// Client-generated unix milliseconds used by the optimistic row and
    /// accepted only when it is close to server time.
    pub created_at: String,
}

#[derive(Debug, Serialize, distributed::GraphqlOutput)]
pub struct ChatPostPayload {
    pub message_id: String,
    pub room_id: String,
    pub author_id: String,
    pub body: String,
    pub created_at: String,
}

pub async fn handle(
    ctx: &CausalCommandContext<'_, ChatMessage>,
    input: ChatPostInput,
) -> Result<PreparedCommand<Causal<ChatPostPayload>>, HandlerError> {
    let author = ctx.user_id()?.to_string();
    let created_at = canonical_near_unix_millis(&input.created_at)?;

    if ctx.load(&input.message_id).await?.is_some() {
        return Err(HandlerError::Rejected(format!(
            "message {} already exists",
            input.message_id
        )));
    }

    let mut msg = ctx.create();
    msg.post(
        &input.message_id,
        &input.room_id,
        &author,
        &input.body,
        &created_at,
    )
    .map_err(rejected)?;

    let state = ChatMessageState::from(&*msg);
    ctx.publish_events().commit(msg)?.causal(ChatPostPayload {
        message_id: state.message_id,
        room_id: state.room_id,
        author_id: state.author_id,
        body: state.body,
        created_at: state.created_at,
    })
}

fn canonical_near_unix_millis(value: &str) -> Result<String, HandlerError> {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let millis = value
        .parse::<u128>()
        .map_err(|_| rejected("created_at must be canonical unix milliseconds"))?;
    if millis.to_string() != value || millis.abs_diff(now) > 300_000 {
        return Err(rejected(
            "created_at must be canonical unix milliseconds within five minutes of server time",
        ));
    }
    Ok(value.to_string())
}
