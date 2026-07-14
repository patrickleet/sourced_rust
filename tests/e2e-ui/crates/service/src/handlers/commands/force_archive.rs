//! Command: `todo.force_archive` — **admin-only** GraphQL mutation.
//!
//! Archives any todo by id without requiring the caller to be the owner.
//! Domain still applies archive via the real `owner_id` on the aggregate
//! (so projectors/events stay consistent). Session must carry role `admin`
//! (enforced in `guard`; GraphQL also registers the field only for admin).

use distributed::microsvc::{Context, HandlerError};
use distributed::OutboxMessage;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use todo_domain::TodoFact;

use crate::deps::TodoDeps;
use crate::handlers::util::{rejected, require_user, session_has_user, session_is_admin};

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

pub fn guard<R, L, S>(ctx: &Context<TodoDeps<R, L, S>>) -> bool
where
    R: crate::bounds::EventStore,
    L: crate::bounds::Locks,
    S: Send + Sync + 'static,
{
    // Session checks belong here: missing admin/user → GuardRejected, not handle body.
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
    // Guard already required admin + user id; extract principal for the payload.
    let admin = require_user(ctx.session())?;
    let input = ctx.input::<TodoForceArchiveInput>()?;

    let mut todo = ctx
        .repo()
        .get(&input.todo_id)
        .await?
        .ok_or_else(|| HandlerError::NotFound(input.todo_id.clone()))?;

    // Domain archive is owner-scoped; use the aggregate's real owner (not admin id).
    let owner = todo.owner_id.clone();
    todo.archive(&owner).map_err(rejected)?;

    let fact = TodoFact::from_todo(&todo);
    let outbox = OutboxMessage::encode(
        format!("{}:todo.archived:{}", todo.todo_id, todo.entity.version()),
        "todo.archived",
        &fact,
    )
    .map_err(|e| HandlerError::Other(Box::new(e)))?;

    ctx.repo().outbox(outbox).commit(&mut todo).await?;

    Ok(json!({
        "todo_id": fact.todo_id,
        "owner_id": fact.owner_id,
        "status": fact.status,
        "archived_by": admin,
    }))
}
