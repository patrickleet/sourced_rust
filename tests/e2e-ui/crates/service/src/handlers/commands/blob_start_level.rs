//! Command: `blob.start_level` — next level after complete (new generated map).

use blob_domain::BlobGame;
use distributed::graphql::{PreparedCommand, Projected};
use distributed::microsvc::{CausalCommandContext, HandlerError};
use e2e_readmodels::BlobGames;
use serde::Deserialize;

use crate::handlers::util::rejected;

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
    let repo = ctx.repo();
    let mut game = repo
        .get(&input.game_id)
        .await?
        .ok_or_else(|| HandlerError::NotFound(input.game_id.clone()))?;
    // Fresh passable layout each level (like original generateLevel)
    game.start_next_generated_level(&owner).map_err(rejected)?;
    // Placement-selected direct: registration owns project_blob / SAVE_BLOB_GAME.
    repo.commit(game)?.projected()
}
