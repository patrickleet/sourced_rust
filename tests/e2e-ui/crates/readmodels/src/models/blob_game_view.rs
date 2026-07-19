use blob_domain::BlobGameFact;
use distributed::ReadModel;
use serde::{Deserialize, Serialize};

use super::AuthUserView;

/// Blob game row. PK: `game_id`. Isolation key: `owner_id`.
/// `map_json` is the projected grid (tile ints); updated only from domain events.
///
/// `owner` is a GraphQL join onto imported [`AuthUserView`] (`owner_id` = `user_id`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ReadModel)]
#[table("blob_games")]
pub struct BlobGameView {
    #[id("game_id")]
    pub game_id: String,
    pub owner_id: String,
    pub score: i64,
    pub player_dead: bool,
    pub current_level: i64,
    pub current_level_completed: bool,
    pub map_json: String,
    /// `active` | `dead` | `level_complete`
    pub status: String,
    #[readmodel(belongs_to = "AuthUserView", foreign_key = "owner_id")]
    pub owner: Option<AuthUserView>,
}

pub fn map_blob_fact(e: &BlobGameFact) -> BlobGameView {
    BlobGameView {
        game_id: e.game_id.clone(),
        owner_id: e.owner_id.clone(),
        score: e.score,
        player_dead: e.player_dead,
        current_level: e.current_level,
        current_level_completed: e.current_level_completed,
        map_json: e.map_json.clone(),
        status: e.status.clone(),
        owner: None,
    }
}
