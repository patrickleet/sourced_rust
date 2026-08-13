//! Handler: counter.increment

use distributed::microsvc::{Context, HandlerError};
use serde::Deserialize;
use serde_json::{json, Value};

use super::{counter_state_message, Repo};
use crate::models::counter::Counter;

pub const COMMAND: &str = "counter.increment";

#[derive(Deserialize)]
pub struct Input {
    pub id: String,
    pub amount: i64,
}

pub fn guard(ctx: &Context<Repo>) -> bool {
    ctx.has_fields(&["id", "amount"])
}

pub async fn handle(ctx: &Context<'_, Repo>) -> Result<Value, HandlerError> {
    let input = ctx.input::<Input>()?;

    let mut counter: Counter = ctx
        .repo()
        .get(&input.id)
        .await?
        .ok_or_else(|| HandlerError::NotFound(input.id.clone()))?;

    counter.increment(input.amount)?;

    let message = counter_state_message(&counter, "counter.incremented")?;

    ctx.repo().outbox(message).commit(&mut counter).await?;

    Ok(json!({ "id": input.id, "value": counter.value }))
}
