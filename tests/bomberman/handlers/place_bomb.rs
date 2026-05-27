use sourced_rust::{
    Aggregate, Commit, Get, GetAggregate, ReadModelWritePlanStore, RelationalReadModelQueryStore,
    SyncReadModelWritePlanCommitExt, TransactionalCommit,
};

use super::shared::{
    board_write_plan, build_board_from_aggregates, load_board, load_board_bombs,
    load_board_explosions, load_board_players,
};
use crate::domain::bomb::Bomb;
use crate::domain::game_map::GameMap;
use crate::domain::player::Player;
use crate::error::GameError;

pub fn place_bomb<R>(repo: &R, player_id: &str, game_id: &str) -> Result<(), GameError>
where
    R: Commit + TransactionalCommit + Get + ReadModelWritePlanStore + RelationalReadModelQueryStore,
{
    let map: GameMap = repo
        .get_aggregate(game_id)?
        .ok_or(GameError::GameNotFound)?;

    let mut player: Player = repo
        .get_aggregate(&format!("player:{}", player_id))?
        .ok_or(GameError::PlayerNotFound(player_id.to_string()))?;

    if !player.alive {
        return Err(GameError::PlayerDead(player_id.to_string()));
    }
    if !player.can_place_bomb() {
        return Err(GameError::NoBombsAvailable);
    }

    let bomb_num = player.bombs_placed + 1;

    player.place_bomb()?;

    let mut bomb = Bomb::default();
    bomb.create(
        format!("bomb:{}:{}", player_id, bomb_num),
        player_id.into(),
        player.x,
        player.y,
        player.blast_radius,
    )?;

    let current_board = load_board(repo, game_id)?;
    let mut all_players: Vec<Player> = load_board_players(repo, &current_board)?
        .into_iter()
        .filter(|existing| existing.entity.id() != player.entity.id())
        .collect();
    all_players.push(player.clone());

    let mut all_bombs: Vec<Bomb> = load_board_bombs(repo, &current_board)?
        .into_iter()
        .filter(|existing| existing.entity.id() != bomb.entity.id())
        .collect();
    all_bombs.push(bomb.clone());

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

    repo.read_models_sync(board_write_plan(&board)?)
        .commit_many_sync(&mut [player.entity_mut(), bomb.entity_mut()])?;

    Ok(())
}
