use sourced_rust::read_model::ReadModelStore;
use sourced_rust::{Commit, Get, ReadModelsExt, Versioned};

use crate::commands;
use crate::domain::player::Player;
use crate::domain::tick_saga::TickSaga;
use crate::domain::types::Direction;
use crate::error::GameError;
use crate::views::BoardView;

pub struct Game<'a, R> {
    repo: &'a R,
    pub game_id: String,
}

impl<'a, R> Game<'a, R>
where
    R: Commit + ReadModelStore + Get + sourced_rust::Find,
{
    pub fn new(repo: &'a R, game_id: &str, ascii: &str) -> Result<Self, GameError> {
        commands::create_game(repo, game_id, ascii)?;
        Ok(Self {
            repo,
            game_id: game_id.to_string(),
        })
    }

    pub fn sim(&self, id: &str, name: &str) -> PlayerSim<'_, R> {
        PlayerSim {
            game: self,
            id: id.to_string(),
            name: name.to_string(),
        }
    }

    pub fn tick(&self) -> Result<TickSaga, GameError> {
        commands::tick(self.repo, &self.game_id)
    }

    pub fn board(&self) -> Result<Versioned<BoardView>, GameError> {
        self.repo
            .read_models::<BoardView>()
            .get(&self.game_id)
            .map_err(|e| GameError::Repository(sourced_rust::RepositoryError::Model(e.to_string())))?
            .ok_or(GameError::GameNotFound)
    }
}

pub struct PlayerSim<'a, R> {
    game: &'a Game<'a, R>,
    pub id: String,
    pub name: String,
}

impl<'a, R> PlayerSim<'a, R>
where
    R: Commit + ReadModelStore + Get + sourced_rust::Find,
{
    pub fn join(&self, spawn_index: usize) -> Result<(), GameError> {
        commands::join_game(self.game.repo, &self.id, &self.name, &self.game.game_id, spawn_index)
    }

    pub fn move_dir(&self, dir: Direction) -> Result<(), GameError> {
        commands::move_player(self.game.repo, &self.id, dir, &self.game.game_id)
    }

    pub fn place_bomb(&self) -> Result<(), GameError> {
        commands::place_bomb(self.game.repo, &self.id, &self.game.game_id)
    }

    pub fn is_alive(&self) -> Result<bool, GameError> {
        let player = self.player()?;
        Ok(player.alive)
    }

    pub fn player(&self) -> Result<Player, GameError> {
        commands::get_player(self.game.repo, &self.id)
    }
}
