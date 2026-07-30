//! Command: `todo.purge` — owner-only physical read-model deletion.

use distributed::graphql::{Causal, PreparedCommand};
use distributed::microsvc::{CausalCommandContext, HandlerError};
use serde::{Deserialize, Serialize};
use todo_domain::Todo;

use crate::handlers::util::rejected;

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
    let repo = ctx.repo();
    let mut todo = repo
        .get(&input.todo_id)
        .await?
        .ok_or_else(|| HandlerError::NotFound(input.todo_id.clone()))?;
    todo.purge(&owner).map_err(rejected)?;

    repo.publish_events()
        .commit(todo)?
        .causal(TodoPurgePayload {
            todo_id: input.todo_id,
            purged: true,
        })
}
