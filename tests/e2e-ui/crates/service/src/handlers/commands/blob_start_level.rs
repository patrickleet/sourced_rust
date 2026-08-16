//! Command: `blob.start_level` — next level after complete (new generated map).

use blob_domain::{BlobGame, BlobGameState};
use distributed::graphql::{PreparedCommand, Atomic};
use distributed::microsvc::{CausalCommandContext, HandlerError};
use e2e_projections::SaveBlobGame;
use e2e_readmodels::BlobGames;
use serde::Deserialize;

use crate::handlers::util::{principal, rejected};

pub const COMMAND: &str = "blob.start_level";

#[derive(Debug, Deserialize, distributed::GraphqlInput)]
pub struct BlobStartLevelInput {
    pub game_id: String,
}

pub async fn handle(
    ctx: &CausalCommandContext<'_, BlobGame>,
    input: BlobStartLevelInput,
) -> Result<PreparedCommand<Atomic<BlobGames>>, HandlerError> {
    let owner = principal(ctx)?;
    let repo = ctx.repo();
    let mut game = repo
        .get(&input.game_id)
        .await?
        .ok_or_else(|| HandlerError::NotFound(input.game_id.clone()))?;
    // Fresh passable layout each level (like original generateLevel)
    game.start_next_generated_level(&owner).map_err(rejected)?;

    let row = SaveBlobGame()
        .from_state(&BlobGameState::from(&*game))
        .map_err(|error| HandlerError::Other(Box::new(error)))?;
    repo.readmodel(row)
        .publish_events()
        .commit(game)?
        .atomic()
}
