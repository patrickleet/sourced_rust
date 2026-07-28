//! Command: `todo.force_archive` — **admin-only** GraphQL mutation.
//!
//! Emits `todo.force_archived` (distinct from owner `todo.archived`) so audit
//! trails can tell admin intervention from self-service archive. Projector
//! still upserts the same read-model shape.

use distributed::graphql::{Causal, PreparedCommand};
use distributed::microsvc::{CausalCommandContext, HandlerError};
use serde::{Deserialize, Serialize};
use todo_domain::Todo;

use crate::handlers::commands::todo_cmd::{load_todo, map_domain, stage_todo_event};

pub const COMMAND: &str = "todo.force_archive";

/// Outbox / projector event name — distinct from owner `todo.archived`.
pub const FORCE_ARCHIVED_EVENT: &str = "todo.force_archived";

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

    // Domain archive is owner-scoped; use the aggregate's real owner (not admin id).
    let owner = todo.owner_id.clone();
    todo.archive(&owner).map_err(map_domain)?;
    let fact = stage_todo_event(ctx, todo, FORCE_ARCHIVED_EVENT)?;

    PreparedCommand::<Causal<TodoForceArchivePayload>>::prepare(TodoForceArchivePayload {
        todo_id: fact.todo_id,
        owner_id: fact.owner_id,
        status: fact.status,
        archived_by: admin,
    })
    .map_err(|error| HandlerError::Other(Box::new(error)))
}
