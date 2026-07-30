//! Command: `blob.move` — direction up|down|left|right.

use blob_domain::{BlobGame, Direction};
use distributed::graphql::{PreparedCommand, Projected};
use distributed::microsvc::{CausalCommandContext, HandlerError};
use e2e_projections::BLOB_GAMES;
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
) -> Result<PreparedCommand<Projected<BlobGames>>, HandlerError> {
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

    repo.project(BLOB_GAMES).commit(game)?.projected()
}
