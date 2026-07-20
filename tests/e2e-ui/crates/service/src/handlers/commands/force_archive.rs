//! Command: `todo.force_archive` — **admin-only** GraphQL mutation.
//!
//! Emits `todo.force_archived` (distinct from owner `todo.archived`) so audit
//! trails can tell admin intervention from self-service archive. Projector
//! still upserts the same read-model shape.

use distributed::microsvc::{Context, HandlerError};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::deps::TodoDeps;
use crate::handlers::commands::todo_cmd::{commit_todo_event, load_todo, map_domain};
use crate::handlers::util::{require_user, session_has_user, session_is_admin};

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

pub fn guard<R, L, S>(ctx: &Context<TodoDeps<R, L, S>>) -> bool
where
    R: crate::bounds::EventStore,
    L: crate::bounds::Locks,
    S: Send + Sync + 'static,
{
    ctx.has_fields(&["todo_id"])
        && session_has_user(ctx.session())
        && session_is_admin(ctx.session())
}

pub async fn handle<R, L, S>(
    ctx: &Context<'_, TodoDeps<R, L, S>>,
) -> Result<Value, HandlerError>
where
    R: crate::bounds::EventStore,
    L: crate::bounds::Locks,
    S: Send + Sync + 'static,
{
    let admin = require_user(ctx.session())?;
    let input = ctx.input::<TodoForceArchiveInput>()?;
    let mut todo = load_todo(ctx, &input.todo_id).await?;

    // Domain archive is owner-scoped; use the aggregate's real owner (not admin id).
    let owner = todo.owner_id.clone();
    todo.archive(owner).map_err(map_domain)?;
    let fact = commit_todo_event(ctx, &mut todo, FORCE_ARCHIVED_EVENT).await?;

    Ok(json!({
        "todo_id": fact.todo_id,
        "owner_id": fact.owner_id,
        "status": fact.status,
        "archived_by": admin,
    }))
}
