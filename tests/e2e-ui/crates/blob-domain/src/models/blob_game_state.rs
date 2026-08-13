use distributed::DomainState;
use serde::{Deserialize, Serialize};

use super::BlobGame;

/// Stable public post-transition state carried by Blob domain events.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, DomainState)]
#[domain_state(version = 1)]
pub struct BlobGameState {
    pub game_id: String,
    pub owner_id: String,
    pub score: i64,
    pub player_dead: bool,
    pub current_level: i64,
    pub current_level_completed: bool,
    /// JSON-encoded `number[][]` of tile ints.
    pub map_json: String,
    /// `active` | `dead` | `level_complete`
    pub status: String,
}

impl BlobGameState {
    /// Capture the aggregate's current public state.
    pub fn from_game(game: &BlobGame) -> Self {
        Self::from(game)
    }
}

impl From<&BlobGame> for BlobGameState {
    fn from(game: &BlobGame) -> Self {
        game.state()
    }
}
