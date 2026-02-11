//! Handler: counter.create
//!
//! Follows the microsvc handler convention:
//! - `COMMAND` — the command name this handler responds to
//! - `guard` — input validation (optional but conventional)
//! - `handle` — the command handler

use serde::Deserialize;
use serde_json::{json, Value};
use sourced_rust::microsvc::{Context, HandlerError};
use sourced_rust::{CommitAggregate, GetAggregate};

use crate::support::Counter;

pub const COMMAND: &str = "counter.create";

#[derive(Deserialize)]
pub struct Input {
    pub id: String,
}

pub fn guard<R>(ctx: &Context<R>) -> bool {
    ctx.has_fields(&["id"])
}

pub fn handle<R: GetAggregate + CommitAggregate>(ctx: &Context<R>) -> Result<Value, HandlerError> {
    let input = ctx.input::<Input>()?;

    // Check if counter already exists
    if ctx.repo().get_aggregate::<Counter>(&input.id)?.is_some() {
        return Err(HandlerError::Rejected(format!(
            "counter {} already exists",
            input.id
        )));
    }

    let mut counter = Counter::default();
    counter.create(input.id.clone());
    ctx.repo().commit_aggregate(&mut counter)?;

    Ok(json!({ "id": input.id }))
}
