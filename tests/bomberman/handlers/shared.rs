use sourced_rust::{
    Aggregate, Get, GetAggregate, ReadModelWorkspaceExt, ReadModelWritePlanBuilder,
    ReadModelWritePlanStore, RelationalReadModelQueryStore, RepositoryError, RowKey, RowValue,
};

use crate::domain::bomb::Bomb;
use crate::domain::explosion::Explosion;
use crate::domain::game_map::GameMap;
use crate::domain::player::Player;
use crate::error::GameError;
use crate::views::{build_board, BoardView};

pub(crate) fn board_key(game_id: &str) -> RowKey {
    RowKey::new([("game_id", RowValue::String(game_id.into()))])
}

pub(crate) fn board_write_plan(board: &BoardView) -> Result<ReadModelWritePlanBuilder, GameError> {
    let mut read_models = ReadModelWritePlanBuilder::new();
    read_models.upsert(board).map_err(RepositoryError::from)?;
    Ok(read_models)
}

pub(crate) fn load_board<R>(repo: &R, game_id: &str) -> Result<BoardView, GameError>
where
    R: ReadModelWritePlanStore + RelationalReadModelQueryStore,
{
    repo.workspace()
        .load::<BoardView>(board_key(game_id))
        .one()
        .map_err(RepositoryError::from)?
        .map(|board| board.data)
        .ok_or(GameError::GameNotFound)
}

fn load_indexed_aggregates<R, A, I>(repo: &R, ids: I) -> Result<Vec<A>, GameError>
where
    R: Get,
    A: Aggregate,
    I: IntoIterator,
    I::Item: AsRef<str>,
{
    let mut results = Vec::new();
    for id in ids {
        let id = id.as_ref();
        let aggregate = repo
            .get_aggregate(id)?
            .ok_or_else(|| RepositoryError::NotFound { id: id.to_string() })?;
        results.push(aggregate);
    }
    Ok(results)
}

pub(crate) fn load_board_players<R: Get>(
    repo: &R,
    board: &BoardView,
) -> Result<Vec<Player>, GameError> {
    load_indexed_aggregates(repo, board.players.iter().map(|player| player.id.as_str()))
}

pub(crate) fn load_board_bombs<R: Get>(
    repo: &R,
    board: &BoardView,
) -> Result<Vec<Bomb>, GameError> {
    load_indexed_aggregates(repo, board.bombs.iter().map(|bomb| bomb.id.as_str()))
}

pub(crate) fn load_board_explosions<R: Get>(
    repo: &R,
    board: &BoardView,
) -> Result<Vec<Explosion>, GameError> {
    load_indexed_aggregates(
        repo,
        board
            .explosions
            .iter()
            .map(|explosion| explosion.id.as_str()),
    )
}

pub(crate) fn build_board_from_aggregates(
    game_id: &str,
    map: &GameMap,
    players: &[Player],
    bombs: &[Bomb],
    explosions: &[Explosion],
    turn: u32,
    explosions_created: u64,
) -> BoardView {
    let player_refs: Vec<&Player> = players.iter().collect();
    let bomb_refs: Vec<&Bomb> = bombs.iter().collect();
    let explosion_refs: Vec<&Explosion> = explosions.iter().collect();

    build_board(
        game_id,
        map,
        &player_refs,
        &bomb_refs,
        &explosion_refs,
        turn,
        explosions_created,
    )
}
