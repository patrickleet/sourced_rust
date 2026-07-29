//! Command: `blob.start` — create game + demo level. Owner = session user.

use blob_domain::{BlobGame, BlobGames};
use distributed::graphql::{PreparedCommand, Projected};
use distributed::microsvc::{CausalCommandContext, HandlerError};
use serde::Deserialize;

use crate::handlers::commands::blob_cmd::{commit_blob, map_domain};

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

    if ctx.load(&input.game_id).await?.is_some() {
        return Err(HandlerError::Rejected(format!(
            "game {} already exists",
            input.game_id
        )));
    }

    let mut game = ctx.create();
    game.start_with_demo(&input.game_id, &owner)
        .map_err(map_domain)?;

    commit_blob(ctx, game)
}
