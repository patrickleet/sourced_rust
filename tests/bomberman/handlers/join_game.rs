use sourced_rust::{
    Commit, Get, GetAggregate, ReadModelWritePlanCommitExt, ReadModelWritePlanStore,
    RelationalReadModelQueryStore, TransactionalCommit,
};

use super::shared::{
    board_write_plan, build_board_from_aggregates, load_board, load_board_bombs,
    load_board_explosions, load_board_players,
};
use crate::domain::game_map::GameMap;
use crate::domain::player::Player;
use crate::error::GameError;

pub fn join_game<R>(
    repo: &R,
    player_id: &str,
    name: &str,
    game_id: &str,
    spawn_index: usize,
) -> Result<(), GameError>
where
    R: Commit + TransactionalCommit + Get + ReadModelWritePlanStore + RelationalReadModelQueryStore,
{
    let map: GameMap = repo
        .get_aggregate(game_id)?
        .ok_or(GameError::GameNotFound)?;

    let (sx, sy) = map.spawn_points[spawn_index];

    let mut player = Player::default();
    player.join(format!("player:{}", player_id), name.into(), sx, sy)?;

    let current_board = load_board(repo, game_id)?;
    let mut all_players = load_board_players(repo, &current_board)?;
    all_players.push(player.clone());

    let all_bombs = load_board_bombs(repo, &current_board)?;
    let all_explosions = load_board_explosions(repo, &current_board)?;
    let board = build_board_from_aggregates(
        game_id,
        &map,
        &all_players,
        &all_bombs,
        &all_explosions,
        current_board.turn,
        current_board.explosions_created,
    );

    repo.read_models(board_write_plan(&board)?)
        .commit(&mut player)?;

    Ok(())
}
