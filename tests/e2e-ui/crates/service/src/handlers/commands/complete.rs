//! Command: `todo.complete` — owner-only (aggregate enforces).

use distributed::graphql::{Causal, PreparedCommand};
use distributed::microsvc::{CausalCommandContext, HandlerError};
use serde::Deserialize;
use todo_domain::Todo;

use crate::handlers::commands::payloads::TodoStatusPayload;
use crate::handlers::commands::todo_cmd::{commit_todo_event, load_todo, map_domain};

pub const COMMAND: &str = "todo.complete";

#[derive(Debug, Deserialize, distributed::GraphqlInput)]
pub struct TodoCompleteInput {
    pub todo_id: String,
}

pub async fn handle(
    ctx: &CausalCommandContext<'_, Todo>,
    input: TodoCompleteInput,
) -> Result<PreparedCommand<Causal<TodoStatusPayload>>, HandlerError> {
    let owner = ctx.user_id()?.to_string();
    let mut todo = load_todo(ctx, &input.todo_id).await?;
    todo.complete(&owner).map_err(map_domain)?;
    commit_todo_event(ctx, todo, "todo.completed", |fact| TodoStatusPayload {
        todo_id: fact.todo_id,
        status: fact.status,
    })
}
