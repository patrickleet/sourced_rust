//! Command: `blob.move` — direction up|down|left|right.

use blob_domain::{BlobGame, BlobGames, Direction};
use distributed::graphql::{PreparedCommand, Projected};
use distributed::microsvc::{CausalCommandContext, HandlerError};
use serde::Deserialize;

use crate::handlers::commands::blob_cmd::{commit_blob, load_game, map_domain};

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

    let mut game = load_game(ctx, &input.game_id).await?;
    game.move_dir(&owner, dir).map_err(map_domain)?;

    commit_blob(ctx, game)
}
