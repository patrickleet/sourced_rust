//! Command: `todo.archive` — owner-only (aggregate enforces).

use distributed::microsvc::{Context, HandlerError};
use distributed::OutboxMessage;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use todo_domain::TodoFact;

use crate::deps::TodoDeps;
use crate::handlers::util::{rejected, require_user, session_has_user};

pub const COMMAND: &str = "todo.archive";

#[derive(Debug, Deserialize, distributed::GraphqlInput)]
pub struct TodoArchiveInput {
    pub todo_id: String,
}

#[derive(Debug, Serialize, distributed::GraphqlOutput)]
pub struct TodoArchivePayload {
    pub todo_id: String,
    pub status: String,
}

pub fn guard<R, L, S>(ctx: &Context<TodoDeps<R, L, S>>) -> bool
where
    R: crate::bounds::EventStore,
    L: crate::bounds::Locks,
    S: Send + Sync + 'static,
{
    ctx.has_fields(&["todo_id"]) && session_has_user(ctx.session())
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
    let input = ctx.input::<TodoArchiveInput>()?;

    let mut todo = ctx
        .repo()
        .get(&input.todo_id)
        .await?
        .ok_or_else(|| HandlerError::NotFound(input.todo_id.clone()))?;

    todo.archive(&owner).map_err(rejected)?;

    let fact = TodoFact::from_todo(&todo);
    let outbox = OutboxMessage::encode(
        format!("{}:todo.archived:{}", todo.todo_id, todo.entity.version()),
        "todo.archived",
        &fact,
    )
    .map_err(|e| HandlerError::Other(Box::new(e)))?;

    ctx.repo().outbox(outbox).commit(&mut todo).await?;

    Ok(json!({
        "todo_id": fact.todo_id,
        "status": fact.status,
    }))
}
