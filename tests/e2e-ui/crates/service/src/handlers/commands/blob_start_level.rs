//! Command: `blob.start_level` — next level after complete (new generated map).

use blob_domain::BlobGame;
use distributed::graphql::{PreparedCommand, Projected};
use distributed::microsvc::{CausalCommandContext, HandlerError};
use e2e_readmodels::BlobGames;
use serde::Deserialize;

use crate::handlers::commands::blob_cmd::{commit_blob, load_game, map_domain};

pub const COMMAND: &str = "blob.start_level";

#[derive(Debug, Deserialize, distributed::GraphqlInput)]
pub struct BlobStartLevelInput {
    pub game_id: String,
}

pub async fn handle(
    ctx: &CausalCommandContext<'_, BlobGame>,
    input: BlobStartLevelInput,
) -> Result<PreparedCommand<Projected<BlobGames>>, HandlerError> {
    let owner = ctx.user_id()?.to_string();
    let mut game = load_game(ctx, &input.game_id).await?;
    // Fresh passable layout each level (like original generateLevel)
    game.start_next_generated_level(&owner)
        .map_err(map_domain)?;
    commit_blob(ctx, game)
}
