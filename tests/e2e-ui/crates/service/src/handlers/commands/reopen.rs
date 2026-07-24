//! Command: `todo.reopen` — owner-only (aggregate enforces).

use distributed::graphql::{Fact, PreparedCommand};
use distributed::microsvc::{CausalCommandContext, HandlerError};
use serde::Deserialize;
use todo_domain::Todo;

use crate::handlers::commands::payloads::TodoStatusPayload;
use crate::handlers::commands::todo_cmd::{load_todo, map_domain, stage_todo_event};

pub const COMMAND: &str = "todo.reopen";

pub type TodoReopenPayload = TodoStatusPayload;

#[derive(Debug, Deserialize, distributed::GraphqlInput)]
pub struct TodoReopenInput {
    pub todo_id: String,
}

pub async fn handle(
    ctx: &CausalCommandContext<'_, Todo>,
    input: TodoReopenInput,
) -> Result<PreparedCommand<Fact<TodoReopenPayload>>, HandlerError> {
    let owner = ctx.user_id()?.to_string();
    let mut todo = load_todo(ctx, &input.todo_id).await?;
    todo.reopen(&owner).map_err(map_domain)?;
    let fact = stage_todo_event(ctx, todo, "todo.reopened")?;
    PreparedCommand::<Fact<TodoReopenPayload>>::prepare(TodoReopenPayload {
        todo_id: fact.todo_id,
        status: fact.status,
    })
    .map_err(|error| HandlerError::Other(Box::new(error)))
}
