//! Command: `todo.complete`.

use distributed::microsvc::{Context, HandlerError};
use distributed::OutboxMessage;
use serde::Deserialize;
use serde_json::{json, Value};
use todo_domain::TodoFact;

use crate::deps::TodoDeps;
use crate::handlers::util::{rejected, require_user};

pub const COMMAND: &str = "todo.complete";

#[derive(Debug, Deserialize)]
pub struct Input {
    pub todo_id: String,
}

pub fn guard<R, L, S>(ctx: &Context<TodoDeps<R, L, S>>) -> bool
where
    R: crate::bounds::EventStore,
    L: crate::bounds::Locks,
    S: Send + Sync + 'static,
{
    ctx.has_fields(&["todo_id"])
}

pub async fn handle<R, L, S>(
    ctx: &Context<'_, TodoDeps<R, L, S>>,
) -> Result<Value, HandlerError>
where
    R: crate::bounds::EventStore,
    L: crate::bounds::Locks,
    S: Send + Sync + 'static,
{
    let owner = require_user(ctx.session())?;
    let input = ctx.input::<Input>()?;

    let mut todo = ctx
        .repo()
        .get(&input.todo_id)
        .await?
        .ok_or_else(|| HandlerError::NotFound(input.todo_id.clone()))?;

    todo.complete(&owner).map_err(rejected)?;

    let fact = TodoFact::from_todo(&todo);
    let outbox = OutboxMessage::encode(
        format!("{}:todo.completed:{}", todo.todo_id, todo.entity.version()),
        "todo.completed",
        &fact,
    )
    .map_err(|e| HandlerError::Other(Box::new(e)))?;

    ctx.repo().outbox(outbox).commit(&mut todo).await?;

    Ok(json!({
        "todo_id": fact.todo_id,
        "status": fact.status,
    }))
}
