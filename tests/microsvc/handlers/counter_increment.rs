//! Handler: counter.increment

use serde::Deserialize;
use serde_json::{json, Value};
use sourced_rust::microsvc::{Context, HandlerError};
use sourced_rust::{CommitAggregate, GetAggregate};

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

pub fn handle<R: GetAggregate + CommitAggregate>(ctx: &Context<R>) -> Result<Value, HandlerError> {
    let input = ctx.input::<Input>()?;

    let mut counter: Counter = ctx
        .repo()
        .get_aggregate(&input.id)?
        .ok_or_else(|| HandlerError::NotFound(input.id.clone()))?;

    counter.increment(input.amount);
    ctx.repo().commit_aggregate(&mut counter)?;

    Ok(json!({ "id": input.id, "value": counter.value }))
}
