//! Command: `todo.rename` — owner-only (aggregate enforces).

use distributed::graphql::{Eventual, PreparedCommand};
use distributed::microsvc::{CausalCommandContext, HandlerError};
use serde::{Deserialize, Serialize};
use todo_domain::{Todo, TodoState};

use crate::handlers::util::rejected;

pub const COMMAND: &str = "todo.rename";

#[derive(Debug, Deserialize, distributed::GraphqlInput)]
pub struct TodoRenameInput {
    pub todo_id: String,
    pub title: String,
}

#[derive(Debug, Serialize, distributed::GraphqlOutput)]
pub struct TodoRenamePayload {
    pub todo_id: String,
    pub title: String,
    pub status: String,
}

pub async fn handle(
    ctx: &CausalCommandContext<'_, Todo>,
    input: TodoRenameInput,
) -> Result<PreparedCommand<Eventual<TodoRenamePayload>>, HandlerError> {
    let owner = ctx.user_id()?.to_string();
    let repo = ctx.repo();
    let mut todo = repo
        .get(&input.todo_id)
        .await?
        .ok_or_else(|| HandlerError::NotFound(input.todo_id.clone()))?;
    todo.rename(&owner, &input.title).map_err(rejected)?;

    let state = TodoState::from(&*todo);
    repo.publish_events()
        .commit(todo)?
        .eventual(TodoRenamePayload {
            todo_id: state.todo_id,
            title: state.title,
            status: state.status,
        })
}
