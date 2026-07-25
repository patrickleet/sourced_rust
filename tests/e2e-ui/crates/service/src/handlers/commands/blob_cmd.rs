//! Shared load + causal staging path for blob game commands.
//!
//! The returned [`BlobGameView`] is staged through
//! [`CausalCommandContext::projected`], so the aggregate, command ledger, and
//! exact projected row commit atomically. The canonical game row has no
//! asynchronous fact consumer or second writer.

use blob_domain::{BlobGame, BlobGameFact};
use distributed::microsvc::{AggregateCheckout, CausalCommandContext, HandlerError};

use crate::handlers::util::rejected;

pub async fn load_game(
    ctx: &CausalCommandContext<'_, BlobGame>,
    game_id: &str,
) -> Result<AggregateCheckout<BlobGame>, HandlerError> {
    ctx.load(game_id)
        .await?
        .ok_or_else(|| HandlerError::NotFound(game_id.to_string()))
}

/// Stage the aggregate for the framework-owned causal commit. The caller maps
/// the returned snapshot into the direct projection.
pub fn stage_blob(
    ctx: &CausalCommandContext<'_, BlobGame>,
    game: AggregateCheckout<BlobGame>,
) -> Result<BlobGameFact, HandlerError> {
    let fact = BlobGameFact::from_game(&game);
    ctx.stage(game)?;
    Ok(fact)
}

pub fn map_domain(err: impl std::fmt::Display) -> HandlerError {
    rejected(err)
}
