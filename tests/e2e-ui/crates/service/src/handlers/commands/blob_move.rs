//! Command: `blob.move` — direction up|down|left|right.
//!
//! Optimistic outcome fields on the input are **client preview fill** (same
//! pattern as chat `created_at` / `message_id`): the generated `.applies`
//! preview maps them into the replica optimistic layer. Authority still comes
//! only from `game_id` + `direction` + domain `move_dir`.

use blob_domain::{BlobGame, BlobGameState, Direction};
use distributed::graphql::{Atomic, PreparedCommand};
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
    /// Optimistic board JSON (`number[][]`) for `.applies` preview only.
    pub map_json: String,
    pub score: i64,
    pub player_dead: bool,
    pub current_level: i64,
    pub current_level_completed: bool,
    /// `active` | `dead` | `level_complete`
    pub status: String,
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
    // Preview fields on `input` are not trusted for authority.
    game.move_dir(&owner, dir).map_err(rejected)?;

    // Handler-owned atomic: same mutation IR as event→mutation bindings.
    let row = save_blob_game()
        .from_state(&BlobGameState::from(&*game))
        .map_err(|error| HandlerError::Other(Box::new(error)))?;
    repo.readmodel(row)
        .publish_events()
        .commit(game)?
        .atomic()
}
