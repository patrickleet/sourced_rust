use std::fmt;

use sourced_rust::RepositoryError;

#[derive(Debug)]
pub enum GameError {
    NotPassable(i32, i32),
    OutOfBounds(i32, i32),
    PlayerDead(String),
    PlayerNotFound(String),
    NoBombsAvailable,
    GameNotFound,
    Repository(RepositoryError),
}

impl fmt::Display for GameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GameError::NotPassable(x, y) => write!(f, "tile ({}, {}) is not passable", x, y),
            GameError::OutOfBounds(x, y) => write!(f, "position ({}, {}) is out of bounds", x, y),
            GameError::PlayerDead(id) => write!(f, "player {} is dead", id),
            GameError::PlayerNotFound(id) => write!(f, "player {} not found", id),
            GameError::NoBombsAvailable => write!(f, "no bombs available"),
            GameError::GameNotFound => write!(f, "game not found"),
            GameError::Repository(e) => write!(f, "repository error: {}", e),
        }
    }
}

impl From<RepositoryError> for GameError {
    fn from(e: RepositoryError) -> Self {
        GameError::Repository(e)
    }
}
