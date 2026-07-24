//! Causally project any `todo.*` fact → `todos` read model.
//!
//! Commands never write read models. Only these projectors do.

use distributed::microsvc::{CausalProjectorContext, HandlerError};
use e2e_readmodels::map_todo_fact;
use todo_domain::TodoFact;

/// Multi-event projector registration name list.
pub const EVENTS: &[&str] = &[
    "todo.created",
    "todo.renamed",
    "todo.completed",
    "todo.reopened",
    "todo.archived",
    // Admin force-archive (distinct audit event; same RM upsert as archived)
    "todo.force_archived",
];

pub async fn handle(ctx: CausalProjectorContext, fact: TodoFact) -> Result<(), HandlerError> {
    let row = map_todo_fact(&fact);
    ctx.project(&row).await?;
    Ok(())
}
