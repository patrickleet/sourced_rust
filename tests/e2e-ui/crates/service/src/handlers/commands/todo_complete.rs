//! Command: `todo.complete` — owner-only (aggregate enforces).

use distributed::graphql::{Eventual, PreparedCommand};
use distributed::microsvc::{invoke_transition, require_loaded, CausalCommandContext, HandlerError};
use serde::Deserialize;
use todo_domain::{Todo, TodoState};

use crate::handlers::commands::payloads::TodoStatusPayload;
use crate::handlers::util::principal;

pub const COMMAND: &str = "todo.complete";

#[derive(Debug, Deserialize, distributed::GraphqlInput)]
pub struct TodoCompleteInput {
    pub todo_id: String,
}

pub async fn handle(
    ctx: &CausalCommandContext<'_, Todo>,
    input: TodoCompleteInput,
) -> Result<PreparedCommand<Eventual<TodoStatusPayload>>, HandlerError> {
    let owner = principal(ctx)?;
    let repo = ctx.repo();
    let mut todo = require_loaded(repo.get(&input.todo_id).await?, input.todo_id.clone())?;
    invoke_transition(&mut todo, |todo| todo.complete(&owner))?;

    let state = TodoState::from(&*todo);
    repo.publish_events()
        .commit(todo)?
        .eventual(TodoStatusPayload {
            todo_id: state.todo_id,
            status: state.status,
        })
}
