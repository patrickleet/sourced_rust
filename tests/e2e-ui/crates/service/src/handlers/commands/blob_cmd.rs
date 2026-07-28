//! Shared load + causal staging path for blob game commands.
//!
//! The returned [`BlobGameView`] uses the narrow direct-read-model proof, so
//! the aggregate, command ledger, and exact projected row commit atomically.
//! The canonical game row has no asynchronous fact consumer or second writer.

use blob_domain::{BlobGame, BlobGameFact};
use distributed::graphql::{PreparedCommand, Projected};
use distributed::microsvc::{
    direct_read_model, AggregateCheckout, CausalCommandContext, HandlerError,
};
use e2e_readmodels::{map_blob_fact, BlobGameView};

use crate::handlers::util::rejected;

pub async fn load_game(
    ctx: &CausalCommandContext<'_, BlobGame>,
    game_id: &str,
) -> Result<AggregateCheckout<BlobGame>, HandlerError> {
    ctx.load(game_id)
        .await?
        .ok_or_else(|| HandlerError::NotFound(game_id.to_string()))
}

/// Prepare the framework-owned commit and its exact one-row direct projection.
pub fn commit_blob(
    ctx: &CausalCommandContext<'_, BlobGame>,
    game: AggregateCheckout<BlobGame>,
) -> Result<PreparedCommand<Projected<BlobGameView>>, HandlerError> {
    let fact = BlobGameFact::from_game(&game);
    ctx.project(direct_read_model::<BlobGameView>())
        .commit(game)?
        .projected(map_blob_fact(&fact))
}

pub fn map_domain(err: impl std::fmt::Display) -> HandlerError {
    rejected(err)
}
