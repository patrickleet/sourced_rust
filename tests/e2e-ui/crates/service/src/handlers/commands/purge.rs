//! Command: `todo.purge` — owner-only physical read-model deletion.

use distributed::graphql::{Causal, PreparedCommand};
use distributed::microsvc::{CausalCommandContext, HandlerError};
use serde::{Deserialize, Serialize};
use todo_domain::Todo;

use crate::handlers::commands::todo_cmd::{load_todo, map_domain};

pub const COMMAND: &str = "todo.purge";

#[derive(Debug, Deserialize, distributed::GraphqlInput)]
pub struct TodoPurgeInput {
    pub todo_id: String,
}

#[derive(Debug, Serialize, distributed::GraphqlOutput)]
pub struct TodoPurgePayload {
    pub todo_id: String,
    pub purged: bool,
}

pub async fn handle(
    ctx: &CausalCommandContext<'_, Todo>,
    input: TodoPurgeInput,
) -> Result<PreparedCommand<Causal<TodoPurgePayload>>, HandlerError> {
    let owner = ctx.user_id()?.to_string();
    let mut todo = load_todo(ctx, &input.todo_id).await?;
    todo.purge(&owner).map_err(map_domain)?;

    ctx.publish_events()
        .commit(todo)?
        .causal(TodoPurgePayload {
            todo_id: input.todo_id,
            purged: true,
        })
}
