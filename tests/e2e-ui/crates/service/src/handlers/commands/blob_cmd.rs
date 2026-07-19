//! Shared load + commit path for blob game commands.
//!
//! After the aggregate commits, we **upsert the read model in the command path**
//! so the GraphQL mutation response is the live projected row. The UI applies
//! that payload immediately — no waiting on eventual projectors, and no
//! re-implementing domain rules in the browser.

use blob_domain::{BlobGame, BlobGameFact};
use distributed::microsvc::{Context, HandlerError};
use distributed::{OutboxMessage, ReadModelWritePlanBuilder};
use e2e_readmodels::map_blob_fact;
use serde_json::{json, Value};

use crate::bounds::{EventStore, Locks, ReadStore};
use crate::deps::BlobDeps;
use crate::handlers::util::{read_model_error, rejected};

pub async fn load_game<R, L, S>(
    ctx: &Context<'_, BlobDeps<R, L, S>>,
    game_id: &str,
) -> Result<BlobGame, HandlerError>
where
    R: EventStore,
    L: Locks,
    S: ReadStore,
{
    ctx.repo()
        .get(game_id)
        .await?
        .ok_or_else(|| HandlerError::NotFound(game_id.to_string()))
}

/// Commit domain event to the event store/outbox, then write the read model
/// so the mutation payload matches what GraphQL queries will see.
pub async fn commit_blob_event<R, L, S>(
    ctx: &Context<'_, BlobDeps<R, L, S>>,
    game: &mut BlobGame,
    event_name: &str,
) -> Result<BlobGameFact, HandlerError>
where
    R: EventStore,
    L: Locks,
    S: ReadStore,
{
    let fact = BlobGameFact::from_game(game);
    let outbox = OutboxMessage::encode(
        format!("{}:{}:{}", game.game_id, event_name, game.entity.version()),
        event_name,
        &fact,
    )
    .map_err(|e| HandlerError::Other(Box::new(e)))?;
    ctx.repo().outbox(outbox).commit(game).await?;

    // Synchronous RM write for command-response UX (projectors remain idempotent).
    let row = map_blob_fact(&fact);
    let store = ctx.read_model_store();
    let mut plan = ReadModelWritePlanBuilder::new();
    plan.upsert(&row).map_err(read_model_error)?;
    plan.commit(store).await.map_err(read_model_error)?;

    Ok(fact)
}

pub fn fact_json(fact: &BlobGameFact) -> Value {
    json!({
        "game_id": fact.game_id,
        "owner_id": fact.owner_id,
        "score": fact.score,
        "player_dead": fact.player_dead,
        "current_level": fact.current_level,
        "current_level_completed": fact.current_level_completed,
        "map_json": fact.map_json,
        "status": fact.status,
    })
}

pub fn map_domain(err: impl std::fmt::Display) -> HandlerError {
    rejected(err)
}
