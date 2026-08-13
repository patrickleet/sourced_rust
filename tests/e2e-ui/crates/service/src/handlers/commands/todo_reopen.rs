//! Command: `todo.reopen` — owner-only (aggregate enforces).

use distributed::graphql::{Eventual, PreparedCommand};
use distributed::microsvc::{CausalCommandContext, HandlerError};
use serde::Deserialize;
use todo_domain::{Todo, TodoState};

use crate::handlers::commands::payloads::TodoStatusPayload;
use crate::handlers::util::rejected;

pub const COMMAND: &str = "todo.reopen";

pub type TodoReopenPayload = TodoStatusPayload;

#[derive(Debug, Deserialize, distributed::GraphqlInput)]
pub struct TodoReopenInput {
    pub todo_id: String,
}

pub async fn handle(
    ctx: &CausalCommandContext<'_, Todo>,
    input: TodoReopenInput,
) -> Result<PreparedCommand<Eventual<TodoReopenPayload>>, HandlerError> {
    let owner = ctx.user_id()?.to_string();
    let repo = ctx.repo();
    let mut todo = repo
        .get(&input.todo_id)
        .await?
        .ok_or_else(|| HandlerError::NotFound(input.todo_id.clone()))?;
    todo.reopen(&owner).map_err(rejected)?;

    let state = TodoState::from(&*todo);
    repo.publish_events()
        .commit(todo)?
        .eventual(TodoReopenPayload {
            todo_id: state.todo_id,
            status: state.status,
        })
}
