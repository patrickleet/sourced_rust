//! Command: `blob.start` — create game + demo level. Owner = session user.

use blob_domain::{BlobGame, BlobGameState};
use distributed::graphql::{PreparedCommand, Atomic};
use distributed::microsvc::{CausalCommandContext, HandlerError};
use e2e_projections::save_blob_game;
use e2e_readmodels::BlobGames;
use serde::Deserialize;

use crate::handlers::util::{principal, rejected};

pub const COMMAND: &str = "blob.start";

#[derive(Debug, Deserialize, distributed::GraphqlInput)]
pub struct BlobStartInput {
    pub game_id: String,
}

pub type BlobGamePayload = BlobGames;

pub async fn handle(
    ctx: &CausalCommandContext<'_, BlobGame>,
    input: BlobStartInput,
) -> Result<PreparedCommand<Atomic<BlobGamePayload>>, HandlerError> {
    let owner = principal(ctx)?;
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

    let row = save_blob_game()
        .from_state(&BlobGameState::from(&*game))
        .map_err(|error| HandlerError::Other(Box::new(error)))?;
    repo.readmodel(row)
        .publish_events()
        .commit(game)?
        .atomic()
}
