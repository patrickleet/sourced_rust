//! Project any `todo.*` fact → `todos` read model.
//!
//! Commands never write read models. Only these projectors do.

use distributed::microsvc::{Context, HandlerError};
use distributed::ReadModelWritePlanBuilder;
use serde_json::{json, Value};
use todo_domain::TodoFact;
use e2e_readmodels::map_fact;

use crate::deps::TodoDeps;
use crate::handlers::util::{decode_payload, read_model_error};

/// Multi-event projector registration name list.
pub const EVENTS: &[&str] = &[
    "todo.created",
    "todo.renamed",
    "todo.completed",
    "todo.reopened",
    "todo.archived",
];

pub fn guard<R, L, S>(_ctx: &Context<TodoDeps<R, L, S>>) -> bool
where
    R: crate::bounds::EventStore,
    L: crate::bounds::Locks,
    S: crate::bounds::ReadStore,
{
    true
}

pub async fn handle<R, L, S>(
    ctx: &Context<'_, TodoDeps<R, L, S>>,
) -> Result<Value, HandlerError>
where
    R: crate::bounds::EventStore,
    L: crate::bounds::Locks,
    S: crate::bounds::ReadStore,
{
    let fact: TodoFact = decode_payload(ctx.message())?;
    let row = map_fact(&fact);
    let store = ctx.read_model_store();
    let mut plan = ReadModelWritePlanBuilder::new();
    plan.upsert(&row).map_err(read_model_error)?;
    plan.commit(store).await.map_err(read_model_error)?;
    Ok(json!({
        "event": ctx.message().name(),
        "todo_id": fact.todo_id,
        "status": fact.status,
    }))
}
