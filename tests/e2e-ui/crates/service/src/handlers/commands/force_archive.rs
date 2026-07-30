//! Command: `todo.force_archive` — **admin-only** GraphQL mutation.
//!
//! Emits `todo.force_archived` (distinct from owner `todo.archived`) so audit
//! trails can tell admin intervention from self-service archive. Projector
//! still upserts the same read-model shape.

use distributed::graphql::{Causal, PreparedCommand};
use distributed::microsvc::{CausalCommandContext, HandlerError};
use serde::{Deserialize, Serialize};
use todo_domain::{Todo, TodoState};

use crate::handlers::util::rejected;

pub const COMMAND: &str = "todo.force_archive";

#[derive(Debug, Deserialize, distributed::GraphqlInput)]
pub struct TodoForceArchiveInput {
    pub todo_id: String,
}

#[derive(Debug, Serialize, distributed::GraphqlOutput)]
pub struct TodoForceArchivePayload {
    pub todo_id: String,
    pub owner_id: String,
    pub status: String,
    /// Session user id of the admin who forced the archive.
    pub archived_by: String,
}

pub async fn handle(
    ctx: &CausalCommandContext<'_, Todo>,
    input: TodoForceArchiveInput,
) -> Result<PreparedCommand<Causal<TodoForceArchivePayload>>, HandlerError> {
    let admin = ctx.user_id()?.to_string();
    let repo = ctx.repo();
    let mut todo = repo
        .get(&input.todo_id)
        .await?
        .ok_or_else(|| HandlerError::NotFound(input.todo_id.clone()))?;

    todo.force_archive().map_err(rejected)?;
    let state = TodoState::from(&*todo);
    repo.publish_events()
        .commit(todo)?
        .causal(TodoForceArchivePayload {
            todo_id: state.todo_id,
            owner_id: state.owner_id,
            status: state.status,
            archived_by: admin,
        })
}
