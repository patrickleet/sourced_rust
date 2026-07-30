//! Command: `todo.create` — owner is always the authenticated session user.
//!
//! GraphQL: exposed as mutation field `todos_create` (roles: user, admin).
//! Owner cannot be spoofed via input — only `require_user(session)` is written.

use distributed::graphql::{Causal, PreparedCommand};
use distributed::microsvc::{CausalCommandContext, HandlerError};
use serde::{Deserialize, Serialize};
use todo_domain::{Todo, TodoState};

use crate::handlers::util::rejected;

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
    let repo = ctx.repo();

    if repo.get(&input.todo_id).await?.is_some() {
        return Err(HandlerError::Rejected(format!(
            "todo {} already exists",
            input.todo_id
        )));
    }

    let mut todo = repo.create();
    todo.create(&input.todo_id, &owner, &input.title)
        .map_err(rejected)?;

    let state = TodoState::from(&*todo);
    repo.publish_events()
        .commit(todo)?
        .causal(TodoCreatePayload {
            todo_id: state.todo_id,
            owner_id: state.owner_id,
            title: state.title,
            status: state.status,
        })
}
