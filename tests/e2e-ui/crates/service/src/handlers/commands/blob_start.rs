//! Command: `blob.start` — create game + demo level. Owner = session user.

use blob_domain::BlobGame;
use distributed::graphql::{PreparedCommand, Projected};
use distributed::microsvc::{CausalCommandContext, HandlerError};
use e2e_readmodels::BlobGames;
use serde::Deserialize;

use crate::handlers::util::rejected;

pub const COMMAND: &str = "blob.start";

#[derive(Debug, Deserialize, distributed::GraphqlInput)]
pub struct BlobStartInput {
    pub game_id: String,
}

pub type BlobGamePayload = BlobGames;

pub async fn handle(
    ctx: &CausalCommandContext<'_, BlobGame>,
    input: BlobStartInput,
) -> Result<PreparedCommand<Projected<BlobGamePayload>>, HandlerError> {
    let owner = ctx.user_id()?.to_string();
    let repo = ctx.repo();

    if repo.get(&input.game_id).await?.is_some() {
        return Err(HandlerError::Rejected(format!(
            "game {} already exists",
            input.game_id
        )));
    }

    let mut game = repo.create();
    game.start_with_demo(&input.game_id, &owner)
        .map_err(rejected)?;

    // Placement-selected direct: registration owns project_blob / SAVE_BLOB_GAME.
    repo.commit(game)?.projected()
}
