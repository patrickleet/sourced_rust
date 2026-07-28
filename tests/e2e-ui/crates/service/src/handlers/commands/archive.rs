//! Command: `todo.archive` — owner-only (aggregate enforces).

use distributed::graphql::{Causal, PreparedCommand};
use distributed::microsvc::{CausalCommandContext, HandlerError};
use serde::Deserialize;
use todo_domain::Todo;

use crate::handlers::commands::payloads::TodoStatusPayload;
use crate::handlers::commands::todo_cmd::{commit_todo_event, load_todo, map_domain};

pub const COMMAND: &str = "todo.archive";

/// GraphQL output — same shape as complete/reopen (shared type).
pub type TodoArchivePayload = TodoStatusPayload;

#[derive(Debug, Deserialize, distributed::GraphqlInput)]
pub struct TodoArchiveInput {
    pub todo_id: String,
}

pub async fn handle(
    ctx: &CausalCommandContext<'_, Todo>,
    input: TodoArchiveInput,
) -> Result<PreparedCommand<Causal<TodoArchivePayload>>, HandlerError> {
    let owner = ctx.user_id()?.to_string();
    let mut todo = load_todo(ctx, &input.todo_id).await?;
    todo.archive(&owner).map_err(map_domain)?;
    commit_todo_event(ctx, todo, "todo.archived", |fact| TodoArchivePayload {
        todo_id: fact.todo_id,
        status: fact.status,
    })
}
