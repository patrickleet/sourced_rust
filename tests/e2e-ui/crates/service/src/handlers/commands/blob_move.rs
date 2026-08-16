//! Command: `blob.move` — direction up|down|left|right.
//!
//! Input is only what the client knows (`game_id` + `direction`). Server domain
//! seals via Atomic. Client may run the declared pure (`blob.simulate_move` /
//! `$lib/blob/simulate-move`) for known-row optimism; that paint is provisional.

use blob_domain::{BlobGame, BlobGameState, Direction};
use distributed::graphql::{Atomic, PreparedCommand};
use distributed::microsvc::{CausalCommandContext, HandlerError};
use e2e_projections::SaveBlobGame;
use e2e_readmodels::BlobGames;
use serde::Deserialize;

use crate::handlers::util::{principal, rejected};

pub const COMMAND: &str = "blob.move";

#[derive(Debug, Deserialize, distributed::GraphqlInput)]
pub struct BlobMoveInput {
    pub game_id: String,
    pub direction: String,
}

pub async fn handle(
    ctx: &CausalCommandContext<'_, BlobGame>,
    input: BlobMoveInput,
) -> Result<PreparedCommand<Atomic<BlobGames>>, HandlerError> {
    let owner = principal(ctx)?;
    let dir = Direction::parse(&input.direction).ok_or_else(|| {
        HandlerError::Rejected(format!(
            "invalid direction `{}` (use up|down|left|right)",
            input.direction
        ))
    })?;

    let repo = ctx.repo();
    let mut game = repo
        .get(&input.game_id)
        .await?
        .ok_or_else(|| HandlerError::NotFound(input.game_id.clone()))?;
    game.move_dir(&owner, dir).map_err(rejected)?;

    let row = SaveBlobGame()
        .from_state(&BlobGameState::from(&*game))
        .map_err(|error| HandlerError::Other(Box::new(error)))?;
    repo.readmodel(row)
        .publish_events()
        .commit(game)?
        .atomic()
}
