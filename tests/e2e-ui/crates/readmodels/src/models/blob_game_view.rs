use blob_domain::BlobGameFact;
use distributed::graphql::{GraphqlOutputType, GraphqlTypeDef, GraphqlTypeField};
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

impl GraphqlOutputType for BlobGameView {
    fn graphql_type() -> GraphqlTypeDef {
        let scalar = |name: &str, type_name: &str| GraphqlTypeField {
            name: name.into(),
            type_name: type_name.into(),
            nullable: false,
            list: false,
            item_nullable: false,
            nested: None,
        };
        GraphqlTypeDef::new(
            // `Projected<BlobGameView>` is a direct replica write. Its output
            // identity must be the retained model identity so the typed
            // Service binder can prove command/result topology without a
            // hand-authored alias.
            "BlobGameView",
            vec![
                scalar("game_id", "String"),
                scalar("owner_id", "String"),
                scalar("score", "BigInt"),
                scalar("player_dead", "Boolean"),
                scalar("current_level", "BigInt"),
                scalar("current_level_completed", "Boolean"),
                scalar("map_json", "String"),
                scalar("status", "String"),
            ],
        )
        .with_type_id(std::any::TypeId::of::<Self>())
    }
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
