//! Command: `todo.complete` — owner-only (aggregate enforces).

use distributed::microsvc::{Context, HandlerError};
use serde::Deserialize;
use serde_json::Value;

use crate::deps::TodoDeps;
use crate::handlers::commands::todo_cmd::{commit_todo_event, load_todo, map_domain, status_json};
use crate::handlers::util::{require_user, session_has_user};

pub const COMMAND: &str = "todo.complete";

#[derive(Debug, Deserialize, distributed::GraphqlInput)]
pub struct TodoCompleteInput {
    pub todo_id: String,
}

pub fn guard<R, L, S>(ctx: &Context<TodoDeps<R, L, S>>) -> bool
where
    R: crate::bounds::EventStore,
    L: crate::bounds::Locks,
    S: Send + Sync + 'static,
{
    ctx.has_fields(&["todo_id"]) && session_has_user(ctx.session())
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
    let input = ctx.input::<TodoCompleteInput>()?;
    let mut todo = load_todo(ctx, &input.todo_id).await?;
    todo.complete(&owner).map_err(map_domain)?;
    let fact = commit_todo_event(ctx, &mut todo, "todo.completed").await?;
    Ok(status_json(&fact))
}
