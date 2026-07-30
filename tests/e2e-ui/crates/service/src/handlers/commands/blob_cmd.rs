//! Shared load + causal staging path for blob game commands.
//!
//! The returned [`BlobGames`] uses the narrow modeled direct proof, so
//! the aggregate, command ledger, and exact projected row commit atomically.
//! The canonical game row has no asynchronous fact consumer or second writer.

use blob_domain::BlobGame;
use distributed::graphql::{PreparedCommand, Projected};
use distributed::microsvc::{AggregateCheckout, CausalCommandContext, HandlerError};
use e2e_readmodels::{BlobGames, BLOB_GAMES};

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
) -> Result<PreparedCommand<Projected<BlobGames>>, HandlerError> {
    ctx.project(BLOB_GAMES).commit(game)?.projected()
}

pub fn map_domain(err: impl std::fmt::Display) -> HandlerError {
    rejected(err)
}
