//! Command: `todo.create` — owner is always the authenticated session user.
//!
//! GraphQL: exposed as mutation field `todos_create` (roles: user, admin).
//! Owner cannot be spoofed via input — only `require_user(session)` is written.

use distributed::graphql::{Causal, PreparedCommand};
use distributed::microsvc::{CausalCommandContext, HandlerError};
use serde::{Deserialize, Serialize};
use todo_domain::Todo;

use crate::handlers::commands::todo_cmd::{map_domain, stage_todo_event};

pub const COMMAND: &str = "todo.create";

/// Mutation / command input — `owner_id` is never accepted from the client.
#[derive(Debug, Deserialize, distributed::GraphqlInput)]
pub struct TodoCreateInput {
    pub todo_id: String,
    pub title: String,
}

/// GraphQL mutation payload for `todos_create`.
#[derive(Debug, Serialize, distributed::GraphqlOutput)]
pub struct TodoCreatePayload {
    pub todo_id: String,
    pub owner_id: String,
    pub title: String,
    pub status: String,
}

pub async fn handle(
    ctx: &CausalCommandContext<'_, Todo>,
    input: TodoCreateInput,
) -> Result<PreparedCommand<Causal<TodoCreatePayload>>, HandlerError> {
    // Owner is always the authenticated principal — not client-supplied.
    let owner = ctx.user_id()?.to_string();

    if ctx.load(&input.todo_id).await?.is_some() {
        return Err(HandlerError::Rejected(format!(
            "todo {} already exists",
            input.todo_id
        )));
    }

    let mut todo = ctx.create();
    todo.create(&input.todo_id, &owner, &input.title)
        .map_err(map_domain)?;

    let fact = stage_todo_event(ctx, todo, "todo.created")?;

    PreparedCommand::<Causal<TodoCreatePayload>>::prepare(TodoCreatePayload {
        todo_id: fact.todo_id,
        owner_id: fact.owner_id,
        title: fact.title,
        status: fact.status,
    })
    .map_err(|error| HandlerError::Other(Box::new(error)))
}
