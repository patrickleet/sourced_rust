//! Command: `todo.complete` — owner-only (aggregate enforces).

use distributed::graphql::{Eventual, PreparedCommand};
use distributed::microsvc::{CausalCommandContext, HandlerError};
use serde::Deserialize;
use todo_domain::{Todo, TodoState};

use crate::handlers::commands::payloads::TodoStatusPayload;
use crate::handlers::util::{principal, rejected};

pub const COMMAND: &str = "todo.complete";

#[derive(Debug, Deserialize, distributed::GraphqlInput)]
pub struct TodoCompleteInput {
    pub todo_id: String,
}

pub async fn handle(
    ctx: &CausalCommandContext<'_, Todo>,
    input: TodoCompleteInput,
) -> Result<PreparedCommand<Eventual<TodoStatusPayload>>, HandlerError> {
    let owner = principal(ctx)?;
    let repo = ctx.repo();
    let mut todo = repo
        .get(&input.todo_id)
        .await?
        .ok_or_else(|| HandlerError::NotFound(input.todo_id.clone()))?;
    todo.complete(&owner).map_err(rejected)?;

    let state = TodoState::from(&*todo);
    repo.publish_events()
        .commit(todo)?
        .eventual(TodoStatusPayload {
            todo_id: state.todo_id,
            status: state.status,
        })
}
