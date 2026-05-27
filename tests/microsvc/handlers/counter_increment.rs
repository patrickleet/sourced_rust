//! Handler: counter.increment

use serde::Deserialize;
use serde_json::{json, Value};
use sourced_rust::microsvc::{Context, HandlerError};
use sourced_rust::{OutboxCommitExt, OutboxMessage};

use super::Repo;
use crate::models::counter::Counter;

pub const COMMAND: &str = "counter.increment";
pub const SPEC: sourced_rust::microsvc::HandlerSpec =
    sourced_rust::microsvc::HandlerSpec::command(COMMAND);

#[derive(Deserialize)]
pub struct Input {
    pub id: String,
    pub amount: i64,
}

pub fn guard(ctx: &Context<Repo>) -> bool {
    ctx.has_fields(&["id", "amount"])
}

pub fn handle(ctx: &Context<Repo>) -> Result<Value, HandlerError> {
    let input = ctx.input::<Input>()?;

    let mut counter: Counter = ctx
        .repo()
        .get(&input.id)?
        .ok_or_else(|| HandlerError::NotFound(input.id.clone()))?;

    counter.increment(input.amount)?;

    let message = OutboxMessage::domain_event("CounterIncremented", &counter)?;

    ctx.repo().outbox(message).commit(&mut counter)?;

    Ok(json!({ "id": input.id, "value": counter.value }))
}
