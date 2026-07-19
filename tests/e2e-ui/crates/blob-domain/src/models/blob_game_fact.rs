use serde::{Deserialize, Serialize};

use super::BlobGame;

/// Full snapshot fact written on every blob event (projector upsert source).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlobGameFact {
    pub game_id: String,
    pub owner_id: String,
    pub score: i64,
    pub player_dead: bool,
    pub current_level: i64,
    pub current_level_completed: bool,
    /// JSON-encoded `number[][]` of tile ints.
    pub map_json: String,
    pub status: String,
}

impl BlobGameFact {
    pub fn from_game(g: &BlobGame) -> Self {
        g.fact()
    }
}
