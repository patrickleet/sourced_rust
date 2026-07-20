//! Command: `todo.create` — owner is always the authenticated session user.
//!
//! GraphQL: exposed as mutation field `todos_create` (roles: user, admin).
//! Owner cannot be spoofed via input — only `require_user(session)` is written.

use distributed::microsvc::{Context, HandlerError};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use todo_domain::Todo;

use crate::deps::TodoDeps;
use crate::handlers::commands::todo_cmd::{commit_todo_event, map_domain};
use crate::handlers::util::{require_user, session_has_user};

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
    // Owner is always the authenticated principal — not client-supplied.
    let owner = require_user(ctx.session())?;
    let input = ctx.input::<TodoCreateInput>()?;

    if ctx.repo().get(&input.todo_id).await?.is_some() {
        return Err(HandlerError::Rejected(format!(
            "todo {} already exists",
            input.todo_id
        )));
    }

    let title = input.title.trim().to_string();
    if title.is_empty() {
        return Err(HandlerError::Rejected("empty title".into()));
    }
    let mut todo = Todo::default();
    todo.create(input.todo_id.clone(), owner, title)
        .map_err(map_domain)?;

    let fact = commit_todo_event(ctx, &mut todo, "todo.created").await?;

    Ok(json!({
        "todo_id": fact.todo_id,
        "owner_id": fact.owner_id,
        "title": fact.title,
        "status": fact.status,
    }))
}
