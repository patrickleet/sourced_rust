//! Command: `todo.rename` — owner-only (aggregate enforces).

use distributed::graphql::{Fact, PreparedCommand};
use distributed::microsvc::{CausalCommandContext, HandlerError};
use serde::{Deserialize, Serialize};
use todo_domain::Todo;

use crate::handlers::commands::todo_cmd::{load_todo, map_domain, stage_todo_event};

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
) -> Result<PreparedCommand<Fact<TodoRenamePayload>>, HandlerError> {
    let owner = ctx.user_id()?.to_string();
    let mut todo = load_todo(ctx, &input.todo_id).await?;
    todo.rename(&owner, &input.title).map_err(map_domain)?;
    let fact = stage_todo_event(ctx, todo, "todo.renamed")?;
    PreparedCommand::<Fact<TodoRenamePayload>>::prepare(TodoRenamePayload {
        todo_id: fact.todo_id,
        title: fact.title,
        status: fact.status,
    })
    .map_err(|error| HandlerError::Other(Box::new(error)))
}
