//! Shared load + causal staging path for blob game commands.
//!
//! The returned [`BlobGameView`] is staged through
//! [`CausalCommandContext::projected`], so the aggregate, outbox fact, command
//! ledger, and exact projected row commit atomically.

use blob_domain::{BlobGame, BlobGameFact};
use distributed::microsvc::{AggregateCheckout, CausalCommandContext, HandlerError};
use distributed::OutboxMessage;

use crate::handlers::util::rejected;

pub async fn load_game(
    ctx: &CausalCommandContext<'_, BlobGame>,
    game_id: &str,
) -> Result<AggregateCheckout<BlobGame>, HandlerError> {
    ctx.load(game_id)
        .await?
        .ok_or_else(|| HandlerError::NotFound(game_id.to_string()))
}

/// Stage a domain event and its aggregate for the framework-owned causal
/// commit. The caller seals the returned fact into the direct projection.
pub fn stage_blob_event(
    ctx: &CausalCommandContext<'_, BlobGame>,
    game: AggregateCheckout<BlobGame>,
    event_name: &str,
) -> Result<BlobGameFact, HandlerError> {
    let fact = BlobGameFact::from_game(&game);
    let payload =
        serde_json::to_vec(&fact).map_err(|error| HandlerError::Other(Box::new(error)))?;
    let outbox = OutboxMessage::create(
        format!("{}:{}:{}", game.game_id, event_name, game.entity.version()),
        event_name,
        payload,
    )
    .map_err(|e| HandlerError::Other(Box::new(e)))?;
    ctx.stage_outbox(outbox)?;
    ctx.stage(game)?;
    Ok(fact)
}

pub fn map_domain(err: impl std::fmt::Display) -> HandlerError {
    rejected(err)
}
