//! Shared load + outbox commit path for todo command handlers.

use distributed::microsvc::{Context, HandlerError};
use distributed::OutboxMessage;
use serde_json::{json, Value};
use todo_domain::{Todo, TodoFact};

use crate::bounds::{EventStore, Locks};
use crate::deps::TodoDeps;
use crate::handlers::util::rejected;

/// Load aggregate by id or NotFound.
pub async fn load_todo<R, L, S>(
    ctx: &Context<'_, TodoDeps<R, L, S>>,
    todo_id: &str,
) -> Result<Todo, HandlerError>
where
    R: EventStore,
    L: Locks,
    S: Send + Sync + 'static,
{
    ctx.repo()
        .get(todo_id)
        .await?
        .ok_or_else(|| HandlerError::NotFound(todo_id.to_string()))
}

/// Encode domain fact to outbox and commit the aggregate in one transaction.
pub async fn commit_todo_event<R, L, S>(
    ctx: &Context<'_, TodoDeps<R, L, S>>,
    todo: &mut Todo,
    event_name: &str,
) -> Result<TodoFact, HandlerError>
where
    R: EventStore,
    L: Locks,
    S: Send + Sync + 'static,
{
    let fact = TodoFact::from_todo(todo);
    let outbox = OutboxMessage::encode(
        format!("{}:{}:{}", todo.todo_id, event_name, todo.entity.version()),
        event_name,
        &fact,
    )
    .map_err(|e| HandlerError::Other(Box::new(e)))?;
    ctx.repo().outbox(outbox).commit(todo).await?;
    Ok(fact)
}

/// JSON body for status-only GraphQL payloads.
pub fn status_json(fact: &TodoFact) -> Value {
    json!({
        "todo_id": fact.todo_id,
        "status": fact.status,
    })
}

/// Map domain error into HandlerError::Rejected.
pub fn map_domain(err: impl std::fmt::Display) -> HandlerError {
    rejected(err)
}
