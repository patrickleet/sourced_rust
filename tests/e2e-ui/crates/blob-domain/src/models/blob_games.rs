use distributed::graphql::{GraphqlOutputType, GraphqlTypeDef, GraphqlTypeField};
use distributed::ReadModel;
use serde::{Deserialize, Serialize};

use super::BlobGameState;

/// Query-oriented Blob game row. The natural plural name infers `blob_games`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ReadModel)]
#[readmodel(primary_key = ["game_id"])]
pub struct BlobGames {
    #[readmodel(id)]
    pub game_id: String,
    pub owner_id: String,
    pub score: i64,
    pub player_dead: bool,
    pub current_level: i64,
    pub current_level_completed: bool,
    pub map_json: String,
    /// `active` | `dead` | `level_complete`
    pub status: String,
}

impl From<&BlobGameState> for BlobGames {
    fn from(state: &BlobGameState) -> Self {
        Self {
            game_id: state.game_id.clone(),
            owner_id: state.owner_id.clone(),
            score: state.score,
            player_dead: state.player_dead,
            current_level: state.current_level,
            current_level_completed: state.current_level_completed,
            map_json: state.map_json.clone(),
            status: state.status.clone(),
        }
    }
}

impl GraphqlOutputType for BlobGames {
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
            "BlobGames",
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
