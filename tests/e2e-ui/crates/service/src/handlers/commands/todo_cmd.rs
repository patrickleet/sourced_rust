//! Shared load + causal staging path for todo command handlers.

use distributed::graphql::{Causal, GraphqlOutputType, PreparedCommand};
use distributed::microsvc::{AggregateCheckout, CausalCommandContext, HandlerError};
use distributed::OutboxMessage;
use serde::Serialize;
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

/// Encode the current outward DTO and prepare one framework-owned causal
/// commit. Task 18 replaces this temporary low-level envelope with a derived
/// typed domain-event contract.
pub fn commit_todo_event<T>(
    ctx: &CausalCommandContext<'_, Todo>,
    todo: AggregateCheckout<Todo>,
    event_name: &str,
    make_payload: impl FnOnce(TodoFact) -> T,
) -> Result<PreparedCommand<Causal<T>>, HandlerError>
where
    T: GraphqlOutputType + Serialize + Send + Sync + 'static,
{
    let fact = TodoFact::from_todo(&todo);
    let payload_bytes =
        serde_json::to_vec(&fact).map_err(|error| HandlerError::Other(Box::new(error)))?;
    let outbox = OutboxMessage::create(
        format!("{}:{}:{}", todo.todo_id, event_name, todo.entity.version()),
        event_name,
        payload_bytes,
    )
    .map_err(|e| HandlerError::Other(Box::new(e)))?;
    ctx.outbox(outbox).commit(todo)?.causal(make_payload(fact))
}

/// Map domain error into HandlerError::Rejected.
pub fn map_domain(err: impl std::fmt::Display) -> HandlerError {
    rejected(err)
}
