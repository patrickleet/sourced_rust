//! Handler: counter.create
//!
//! Follows the microsvc handler convention:
//! - `COMMAND` — the command name this handler responds to
//! - `guard` — input validation (optional but conventional)
//! - `handle` — the command handler

use serde::Deserialize;
use serde_json::{json, Value};
use sourced_rust::microsvc::{Context, HandlerError};
use sourced_rust::{OutboxMessage, SyncOutboxCommitExt};

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

pub fn handle(ctx: &Context<Repo>) -> Result<Value, HandlerError> {
    let input = ctx.input::<Input>()?;

    if ctx.repo().get(&input.id)?.is_some() {
        return Err(HandlerError::Rejected(format!(
            "counter {} already exists",
            input.id
        )));
    }

    let mut counter = Counter::default();
    counter.create(input.id.clone())?;

    let message = OutboxMessage::domain_event("CounterCreated", &counter)?;

    ctx.repo().outbox_sync(message).commit_sync(&mut counter)?;

    Ok(json!({ "id": input.id }))
}
