//! Command: `todo.force_archive` — **admin-only** GraphQL mutation.
//!
//! Emits `todo.force_archived` (distinct from owner `todo.archived`) so audit
//! trails can tell admin intervention from self-service archive. Projector
//! still upserts the same read-model shape.

use distributed::graphql::{Causal, PreparedCommand};
use distributed::microsvc::{CausalCommandContext, HandlerError};
use serde::{Deserialize, Serialize};
use todo_domain::Todo;

use crate::handlers::commands::todo_cmd::{commit_todo_events, load_todo, map_domain};

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
    let mut todo = load_todo(ctx, &input.todo_id).await?;

    todo.force_archive().map_err(map_domain)?;
    commit_todo_events(ctx, todo, |state| TodoForceArchivePayload {
        todo_id: state.todo_id,
        owner_id: state.owner_id,
        status: state.status,
        archived_by: admin,
    })
}
