//! Causally project `chat_message.posted` → `chat_messages`.

use chat_domain::ChatMessagePosted;
use distributed::microsvc::{CausalProjectorContext, HandlerError};
use e2e_readmodels::map_chat_posted;

pub const EVENT: &str = "chat_message.posted";

pub async fn handle(
    ctx: CausalProjectorContext,
    fact: ChatMessagePosted,
) -> Result<(), HandlerError> {
    let row = map_chat_posted(&fact);
    ctx.project(&row).await?;
    Ok(())
}
