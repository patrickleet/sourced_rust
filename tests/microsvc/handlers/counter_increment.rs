//! Handler: counter.increment

use serde::Deserialize;
use serde_json::{json, Value};
use sourced_rust::microsvc::{Context, HandlerError};
use sourced_rust::{AggregateBuilder, OutboxCommitExt, OutboxMessage, Repository};

use crate::support::Counter;

pub const COMMAND: &str = "counter.increment";

#[derive(Deserialize)]
pub struct Input {
    pub id: String,
    pub amount: i64,
}

pub fn guard<R>(ctx: &Context<R>) -> bool {
    ctx.has_fields(&["id", "amount"])
}

pub fn handle<R: Repository + Clone>(
    ctx: &Context<R>,
) -> Result<Value, HandlerError> {
    let input = ctx.input::<Input>()?;
    let counter_repo = ctx.repo().clone().aggregate::<Counter>();

    let mut counter: Counter = counter_repo
        .get(&input.id)?
        .ok_or_else(|| HandlerError::NotFound(input.id.clone()))?;

    counter.increment(input.amount);

    let payload = serde_json::to_vec(&json!({ "id": input.id, "amount": input.amount, "value": counter.value }))
        .map_err(|e| HandlerError::Other(Box::new(e)))?;
    let mut message = OutboxMessage::create(
        format!("{}:incremented", input.id),
        "CounterIncremented",
        payload,
    );

    counter_repo.outbox(&mut message).commit(&mut counter)?;

    Ok(json!({ "id": input.id, "value": counter.value }))
}
