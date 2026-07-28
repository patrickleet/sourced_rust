//! Shared load + causal staging path for todo command handlers.

use distributed::graphql::{Causal, GraphqlOutputType, PreparedCommand};
use distributed::microsvc::{AggregateCheckout, CausalCommandContext, HandlerError};
use serde::Serialize;
use todo_domain::projection_v2::TodoState;
use todo_domain::Todo;

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

/// Prepare one framework-owned causal commit from captured domain events.
pub fn commit_todo_events<T>(
    ctx: &CausalCommandContext<'_, Todo>,
    todo: AggregateCheckout<Todo>,
    make_payload: impl FnOnce(TodoState) -> T,
) -> Result<PreparedCommand<Causal<T>>, HandlerError>
where
    T: GraphqlOutputType + Serialize + Send + Sync + 'static,
{
    let state = TodoState::from(&*todo);
    ctx.publish_events()
        .commit(todo)?
        .causal(make_payload(state))
}

/// Map domain error into HandlerError::Rejected.
pub fn map_domain(err: impl std::fmt::Display) -> HandlerError {
    rejected(err)
}
