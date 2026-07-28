//! Command: `todo.rename` — owner-only (aggregate enforces).

use distributed::graphql::{Causal, PreparedCommand};
use distributed::microsvc::{CausalCommandContext, HandlerError};
use serde::{Deserialize, Serialize};
use todo_domain::Todo;

use crate::handlers::commands::todo_cmd::{commit_todo_events, load_todo, map_domain};

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
) -> Result<PreparedCommand<Causal<TodoRenamePayload>>, HandlerError> {
    let owner = ctx.user_id()?.to_string();
    let mut todo = load_todo(ctx, &input.todo_id).await?;
    todo.rename(&owner, &input.title).map_err(map_domain)?;
    commit_todo_events(ctx, todo, |state| TodoRenamePayload {
        todo_id: state.todo_id,
        title: state.title,
        status: state.status,
    })
}
