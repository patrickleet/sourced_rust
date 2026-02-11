//! Handler: counter.create
//!
//! Follows the microsvc handler convention:
//! - `COMMAND` — the command name this handler responds to
//! - `guard` — input validation (optional but conventional)
//! - `handle` — the command handler

use serde::Deserialize;
use serde_json::{json, Value};
use sourced_rust::microsvc::{Context, HandlerError};
use sourced_rust::{AggregateBuilder, OutboxCommitExt, OutboxMessage, Repository};

use crate::models::counter::Counter;

pub const COMMAND: &str = "counter.create";

#[derive(Deserialize)]
pub struct Input {
    pub id: String,
}

pub fn guard<R>(ctx: &Context<R>) -> bool {
    ctx.has_fields(&["id"])
}

pub fn handle<R: Repository + Clone>(
    ctx: &Context<R>,
) -> Result<Value, HandlerError> {
    let input = ctx.input::<Input>()?;
    let counter_repo = ctx.repo().clone().aggregate::<Counter>();

    if counter_repo.get(&input.id)?.is_some() {
        return Err(HandlerError::Rejected(format!(
            "counter {} already exists",
            input.id
        )));
    }

    let mut counter = Counter::default();
    counter.create(input.id.clone());

    let mut message = OutboxMessage::encode(
        format!("{}:created", input.id),
        "CounterCreated",
        &counter.snapshot(),
    )
    .map_err(|e| HandlerError::Other(Box::new(e)))?;

    counter_repo.outbox(&mut message).commit(&mut counter)?;

    Ok(json!({ "id": input.id }))
}
