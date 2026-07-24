//! Command: `blob.move` — direction up|down|left|right.

use blob_domain::{BlobGame, Direction};
use distributed::graphql::{PreparedCommand, Projected};
use distributed::microsvc::{CausalCommandContext, HandlerError};
use e2e_readmodels::{map_blob_fact, BlobGameView};
use serde::Deserialize;

use crate::handlers::commands::blob_cmd::{load_game, map_domain, stage_blob_event};

pub const COMMAND: &str = "blob.move";

#[derive(Debug, Deserialize, distributed::GraphqlInput)]
pub struct BlobMoveInput {
    pub game_id: String,
    pub direction: String,
}

pub async fn handle(
    ctx: &CausalCommandContext<'_, BlobGame>,
    input: BlobMoveInput,
) -> Result<PreparedCommand<Projected<BlobGameView>>, HandlerError> {
    let owner = ctx.user_id()?.to_string();
    let dir = Direction::parse(&input.direction).ok_or_else(|| {
        HandlerError::Rejected(format!(
            "invalid direction `{}` (use up|down|left|right)",
            input.direction
        ))
    })?;

    let mut game = load_game(ctx, &input.game_id).await?;
    game.move_dir(&owner, dir).map_err(map_domain)?;

    let fact = stage_blob_event(ctx, game, "blob.moved")?;
    ctx.projected(map_blob_fact(&fact))
}
