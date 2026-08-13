//! Command: `todo.create` — owner is always the authenticated session user.
//!
//! GraphQL: `todos_create` (roles: user, admin). Session admission is the
//! mount guard (`causal_has_user`); this body binds that principal as owner.

use distributed::graphql::{Eventual, PreparedCommand};
use distributed::microsvc::{CausalCommandContext, HandlerError};
use serde::{Deserialize, Serialize};
use todo_domain::{Todo, TodoState};

use crate::handlers::util::{principal, rejected};

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
) -> Result<PreparedCommand<Eventual<TodoCreatePayload>>, HandlerError> {
    // Owner is always the authenticated principal — not client-supplied.
    let owner = principal(ctx)?;
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
        .eventual(TodoCreatePayload {
            todo_id: state.todo_id,
            owner_id: state.owner_id,
            title: state.title,
            status: state.status,
        })
}
