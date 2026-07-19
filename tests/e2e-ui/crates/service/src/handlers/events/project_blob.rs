//! Project `blob.*` facts → `blob_games` read model.
//! Commands never write this table.

use blob_domain::BlobGameFact;
use distributed::microsvc::{Context, HandlerError};
use distributed::ReadModelWritePlanBuilder;
use e2e_readmodels::map_blob_fact;
use serde_json::{json, Value};

use crate::deps::BlobDeps;
use crate::handlers::util::{decode_payload, read_model_error};

pub const EVENTS: &[&str] = &[
    "blob.started",
    "blob.initialized",
    "blob.level_started",
    "blob.moved",
];

pub fn guard<R, L, S>(_ctx: &Context<BlobDeps<R, L, S>>) -> bool
where
    R: crate::bounds::EventStore,
    L: crate::bounds::Locks,
    S: crate::bounds::ReadStore,
{
    true
}

pub async fn handle<R, L, S>(
    ctx: &Context<'_, BlobDeps<R, L, S>>,
) -> Result<Value, HandlerError>
where
    R: crate::bounds::EventStore,
    L: crate::bounds::Locks,
    S: crate::bounds::ReadStore,
{
    let fact: BlobGameFact = decode_payload(ctx.message())?;
    let row = map_blob_fact(&fact);
    let store = ctx.read_model_store();
    let mut plan = ReadModelWritePlanBuilder::new();
    plan.upsert(&row).map_err(read_model_error)?;
    plan.commit(store).await.map_err(read_model_error)?;
    Ok(json!({
        "event": ctx.message().name(),
        "game_id": fact.game_id,
        "score": fact.score,
        "status": fact.status,
    }))
}
