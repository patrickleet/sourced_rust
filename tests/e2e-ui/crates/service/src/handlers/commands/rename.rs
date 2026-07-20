//! Command: `todo.rename` — owner-only (aggregate enforces).

use distributed::microsvc::{Context, HandlerError};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::deps::TodoDeps;
use crate::handlers::commands::todo_cmd::{commit_todo_event, load_todo, map_domain};
use crate::handlers::util::{require_user, session_has_user};

pub const COMMAND: &str = "todo.rename";

#[derive(Debug, Deserialize, distributed::GraphqlInput)]
pub struct TodoRenameInput {
    pub todo_id: String,
    pub title: String,
}

#[derive(Debug, Serialize, distributed::GraphqlOutput)]
pub struct TodoRenamePayload {
    pub todo_id: String,
    pub title: String,
    pub status: String,
}

pub fn guard<R, L, S>(ctx: &Context<TodoDeps<R, L, S>>) -> bool
where
    R: crate::bounds::EventStore,
    L: crate::bounds::Locks,
    S: Send + Sync + 'static,
{
    ctx.has_fields(&["todo_id", "title"]) && session_has_user(ctx.session())
}

pub async fn handle<R, L, S>(
    ctx: &Context<'_, TodoDeps<R, L, S>>,
) -> Result<Value, HandlerError>
where
    R: crate::bounds::EventStore,
    L: crate::bounds::Locks,
    S: Send + Sync + 'static,
{
    let owner = require_user(ctx.session())?;
    let input = ctx.input::<TodoRenameInput>()?;
    let mut todo = load_todo(ctx, &input.todo_id).await?;
    let title = input.title.trim().to_string();
    if title.is_empty() {
        return Err(HandlerError::Rejected("empty title".into()));
    }
    todo.ensure_owner(&owner).map_err(map_domain)?;
    todo.rename(owner, title).map_err(map_domain)?;
    let fact = commit_todo_event(ctx, &mut todo, "todo.renamed").await?;
    Ok(json!({
        "todo_id": fact.todo_id,
        "title": fact.title,
        "status": fact.status,
    }))
}
