//! Project `chat_message.posted` → `chat_messages` (drives GraphQL subscriptions).

use chat_domain::ChatMessagePosted;
use distributed::microsvc::{Context, HandlerError};
use distributed::ReadModelWritePlanBuilder;
use e2e_readmodels::map_chat_posted;
use serde_json::{json, Value};

use crate::deps::ChatDeps;
use crate::handlers::util::{decode_payload, read_model_error};

pub const EVENT: &str = "chat_message.posted";

pub fn guard<R, L, S>(_ctx: &Context<ChatDeps<R, L, S>>) -> bool
where
    R: crate::bounds::EventStore,
    L: crate::bounds::Locks,
    S: crate::bounds::ReadStore,
{
    true
}

pub async fn handle<R, L, S>(
    ctx: &Context<'_, ChatDeps<R, L, S>>,
) -> Result<Value, HandlerError>
where
    R: crate::bounds::EventStore,
    L: crate::bounds::Locks,
    S: crate::bounds::ReadStore,
{
    let fact: ChatMessagePosted = decode_payload(ctx.message())?;
    let row = map_chat_posted(&fact);
    let store = ctx.read_model_store();
    let mut plan = ReadModelWritePlanBuilder::new();
    plan.upsert(&row).map_err(read_model_error)?;
    plan.commit(store).await.map_err(read_model_error)?;
    Ok(json!({
        "event": EVENT,
        "message_id": fact.message_id,
        "room_id": fact.room_id,
    }))
}
