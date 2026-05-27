use sourced_rust::{
    Commit, Get, ReadModelWorkspaceExt, ReadModelWritePlanStore, RelationalReadModelQueryStore,
    RepositoryError, RowKey, RowValue, TransactionalCommit, Versioned,
};

use crate::domain::player::Player;
use crate::domain::tick_saga::TickSaga;
use crate::domain::types::Direction;
use crate::error::GameError;
use crate::handlers;
use crate::views::BoardView;

pub struct Game<'a, R> {
    repo: &'a R,
    pub game_id: String,
}

impl<'a, R> Game<'a, R>
where
    R: Commit + TransactionalCommit + Get + ReadModelWritePlanStore + RelationalReadModelQueryStore,
{
    pub fn new(repo: &'a R, game_id: &str, ascii: &str) -> Result<Self, GameError> {
        handlers::create_game(repo, game_id, ascii)?;
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
        handlers::tick(self.repo, &self.game_id)
    }

    pub fn board(&self) -> Result<Versioned<BoardView>, GameError> {
        self.repo
            .workspace()
            .load::<BoardView>(RowKey::new([(
                "game_id",
                RowValue::String(self.game_id.clone()),
            )]))
            .one()
            .map_err(|e| GameError::Repository(RepositoryError::Model(e.to_string())))?
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
    R: Commit + TransactionalCommit + Get + ReadModelWritePlanStore + RelationalReadModelQueryStore,
{
    pub fn join(&self, spawn_index: usize) -> Result<(), GameError> {
        handlers::join_game(
            self.game.repo,
            &self.id,
            &self.name,
            &self.game.game_id,
            spawn_index,
        )
    }

    pub fn move_dir(&self, dir: Direction) -> Result<(), GameError> {
        handlers::move_player(self.game.repo, &self.id, dir, &self.game.game_id)
    }

    pub fn place_bomb(&self) -> Result<(), GameError> {
        handlers::place_bomb(self.game.repo, &self.id, &self.game.game_id)
    }

    pub fn is_alive(&self) -> Result<bool, GameError> {
        let player = self.player()?;
        Ok(player.alive)
    }

    pub fn player(&self) -> Result<Player, GameError> {
        handlers::get_player(self.game.repo, &self.id)
    }
}
