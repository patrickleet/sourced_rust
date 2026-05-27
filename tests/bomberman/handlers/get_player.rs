use sourced_rust::{Get, GetAggregate};

use crate::domain::player::Player;
use crate::error::GameError;

pub fn get_player<R: Get>(repo: &R, player_id: &str) -> Result<Player, GameError> {
    repo.get_aggregate(&format!("player:{}", player_id))?
        .ok_or(GameError::PlayerNotFound(player_id.to_string()))
}
