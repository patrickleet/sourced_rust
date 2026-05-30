use sourced_rust::AsyncGetStream;

use super::shared::get_aggregate;
use crate::domain::player::Player;
use crate::error::GameError;

pub async fn get_player<R: AsyncGetStream>(repo: &R, player_id: &str) -> Result<Player, GameError> {
    get_aggregate::<R, Player>(repo, &format!("player:{}", player_id))
        .await?
        .ok_or(GameError::PlayerNotFound(player_id.to_string()))
}
