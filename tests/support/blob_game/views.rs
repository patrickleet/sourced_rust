//! Projection views for BlobGame - optimized for API consumption.

use serde::{Deserialize, Serialize};
use sourced_rust::ProjectionSchema;

use super::aggregate::BlobGame;

/// Game status for projections
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum GameStatus {
    InProgress,
    Won,
    Lost,
}

/// A read-optimized view of a BlobGame for API responses.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BlobGameView {
    pub id: String,
    pub address: String,
    pub score: u32,
    pub current_level: u32,
    pub status: GameStatus,
    pub timed: bool,
}

impl ProjectionSchema for BlobGameView {
    const PREFIX: &'static str = "game_view";

    fn id(&self) -> &str {
        &self.id
    }
}

impl BlobGameView {
    pub fn new(id: impl Into<String>, address: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            address: address.into(),
            score: 0,
            current_level: 0,
            status: GameStatus::InProgress,
            timed: false,
        }
    }

    pub fn update_from(&mut self, game: &BlobGame) {
        self.score = game.score();
        self.current_level = game.current_level().map(|l| l.id).unwrap_or(0);
        self.status = if game.is_player_dead() {
            GameStatus::Lost
        } else if game.is_current_level_completed()
            && game.current_level().map_or(false, |l| l.completed)
        {
            GameStatus::Won
        } else {
            GameStatus::InProgress
        };
    }
}

/// An index of all games a player has participated in.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlayerGamesIndex {
    pub address: String,
    pub game_ids: Vec<String>,
    pub total_score: u32,
    pub games_won: u32,
    pub games_lost: u32,
}

impl ProjectionSchema for PlayerGamesIndex {
    const PREFIX: &'static str = "player_games";

    fn id(&self) -> &str {
        &self.address
    }
}

impl PlayerGamesIndex {
    pub fn new(address: impl Into<String>) -> Self {
        Self {
            address: address.into(),
            game_ids: vec![],
            total_score: 0,
            games_won: 0,
            games_lost: 0,
        }
    }

    pub fn add_game(&mut self, game_id: &str, score: u32, status: &GameStatus) {
        if !self.game_ids.contains(&game_id.to_string()) {
            self.game_ids.push(game_id.to_string());
        }
        self.total_score += score;
        match status {
            GameStatus::Won => self.games_won += 1,
            GameStatus::Lost => self.games_lost += 1,
            GameStatus::InProgress => {}
        }
    }
}
