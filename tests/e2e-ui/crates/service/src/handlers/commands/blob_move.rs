//! Command: `blob.move` — direction up|down|left|right.

use blob_domain::{BlobGame, BlobGameState, Direction};
use distributed::graphql::{PreparedCommand, Atomic};
use distributed::microsvc::{CausalCommandContext, HandlerError};
use e2e_projections::save_blob_game;
use e2e_readmodels::BlobGames;
use serde::Deserialize;

use crate::handlers::util::rejected;

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
    let owner = ctx.user_id()?.to_string();
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

    // Handler-owned projected: same mutation IR as event→mutation bindings.
    let row = save_blob_game()
        .from_state(&BlobGameState::from(&*game))
        .map_err(|error| HandlerError::Other(Box::new(error)))?;
    repo.readmodel(row)
        .publish_events()
        .commit(game)?
        .atomic()
}
