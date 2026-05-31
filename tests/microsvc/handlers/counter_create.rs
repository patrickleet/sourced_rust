//! Handler: counter.create
//!
//! Follows the microsvc handler convention:
//! - `COMMAND` — the command name this handler responds to
//! - `guard` — input validation (optional but conventional)
//! - `handle` — the command handler

use distributed::microsvc::{Context, HandlerError};
use distributed::OutboxMessage;
use serde::Deserialize;
use serde_json::{json, Value};

use super::Repo;
use crate::models::counter::Counter;

pub const COMMAND: &str = "counter.create";

#[derive(Deserialize)]
pub struct Input {
    pub id: String,
}

pub fn guard(ctx: &Context<Repo>) -> bool {
    ctx.has_fields(&["id"])
}

pub async fn handle(ctx: &Context<'_, Repo>) -> Result<Value, HandlerError> {
    let input = ctx.input::<Input>()?;

    if ctx.repo().get(&input.id).await?.is_some() {
        return Err(HandlerError::Rejected(format!(
            "counter {} already exists",
            input.id
        )));
    }

    let mut counter = Counter::default();
    counter.create(input.id.clone())?;

    let message = OutboxMessage::domain_event("CounterCreated", &counter)?;

    ctx.repo().outbox(message).commit(&mut counter).await?;

    Ok(json!({ "id": input.id }))
}
