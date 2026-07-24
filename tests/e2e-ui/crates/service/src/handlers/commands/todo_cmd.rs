//! Shared load + causal staging path for todo command handlers.

use distributed::microsvc::{AggregateCheckout, CausalCommandContext, HandlerError};
use distributed::OutboxMessage;
use todo_domain::{Todo, TodoFact};

use crate::handlers::util::rejected;

/// Load aggregate by id or NotFound.
pub async fn load_todo(
    ctx: &CausalCommandContext<'_, Todo>,
    todo_id: &str,
) -> Result<AggregateCheckout<Todo>, HandlerError> {
    ctx.load(todo_id)
        .await?
        .ok_or_else(|| HandlerError::NotFound(todo_id.to_string()))
}

/// Encode the domain fact and stage both it and the aggregate for one
/// framework-owned causal commit.
pub fn stage_todo_event(
    ctx: &CausalCommandContext<'_, Todo>,
    todo: AggregateCheckout<Todo>,
    event_name: &str,
) -> Result<TodoFact, HandlerError> {
    let fact = TodoFact::from_todo(&todo);
    let payload =
        serde_json::to_vec(&fact).map_err(|error| HandlerError::Other(Box::new(error)))?;
    let outbox = OutboxMessage::create(
        format!("{}:{}:{}", todo.todo_id, event_name, todo.entity.version()),
        event_name,
        payload,
    )
    .map_err(|e| HandlerError::Other(Box::new(e)))?;
    ctx.stage_outbox(outbox)?;
    ctx.stage(todo)?;
    Ok(fact)
}

/// Map domain error into HandlerError::Rejected.
pub fn map_domain(err: impl std::fmt::Display) -> HandlerError {
    rejected(err)
}
