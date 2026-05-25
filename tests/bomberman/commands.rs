use sourced_rust::read_model::ReadModelStore;
use sourced_rust::{
    Aggregate, Commit, CommitBuilderExt, Get, GetAggregate, OutboxMessage, ReadModelsExt,
    RepositoryError, TransactionalCommit,
};

use crate::domain::bomb::Bomb;
use crate::domain::explosion::Explosion;
use crate::domain::game_map::GameMap;
use crate::domain::player::Player;
use crate::domain::tick_saga::{Detonation, TickSaga};
use crate::domain::types::{Direction, Tile};
use crate::error::GameError;
use crate::views::{build_board, BoardView};

#[derive(Default)]
struct DamageReport {
    blocks_destroyed: Vec<(i32, i32)>,
    players_killed: Vec<String>,
    chain_detonations: Vec<String>,
}

impl DamageReport {
    fn has_damage(&self) -> bool {
        !self.blocks_destroyed.is_empty()
            || !self.players_killed.is_empty()
            || !self.chain_detonations.is_empty()
    }
}

struct KillAttribution {
    player_id: String,
    bomb_id: String,
    bomb_owner: String,
}

fn record_kill_attributions(
    attributions: &mut Vec<KillAttribution>,
    killed_ids: &[String],
    bomb_id: &str,
    bomb_owner: &str,
) {
    attributions.extend(killed_ids.iter().map(|player_id| KillAttribution {
        player_id: player_id.clone(),
        bomb_id: bomb_id.to_string(),
        bomb_owner: bomb_owner.to_string(),
    }));
}

fn load_board<R: ReadModelStore>(repo: &R, game_id: &str) -> Result<BoardView, GameError> {
    repo.read_models::<BoardView>()
        .get_by_primary_key(game_id)
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

fn load_board_players<R: Get>(repo: &R, board: &BoardView) -> Result<Vec<Player>, GameError> {
    load_indexed_aggregates(repo, board.players.iter().map(|player| player.id.as_str()))
}

fn load_board_bombs<R: Get>(repo: &R, board: &BoardView) -> Result<Vec<Bomb>, GameError> {
    load_indexed_aggregates(repo, board.bombs.iter().map(|bomb| bomb.id.as_str()))
}

fn load_board_explosions<R: Get>(repo: &R, board: &BoardView) -> Result<Vec<Explosion>, GameError> {
    load_indexed_aggregates(
        repo,
        board
            .explosions
            .iter()
            .map(|explosion| explosion.id.as_str()),
    )
}

// ── Game commands ──

pub fn create_game<R: Commit + ReadModelStore + TransactionalCommit>(
    repo: &R,
    game_id: &str,
    ascii_map: &str,
) -> Result<GameMap, GameError> {
    let (width, height, tiles, spawn_points) = GameMap::from_ascii(ascii_map);

    let mut map = GameMap::default();
    map.create(game_id.into(), width, height, tiles.clone(), spawn_points)?;

    let board = BoardView::new(game_id, width, height, tiles);
    repo.readmodel(&board).commit(&mut map)?;

    Ok(map)
}

pub fn tick<R: Commit + ReadModelStore + TransactionalCommit + Get>(
    repo: &R,
    game_id: &str,
) -> Result<TickSaga, GameError> {
    let mut map: GameMap = repo
        .get_aggregate(game_id)?
        .ok_or(GameError::GameNotFound)?;

    let board = load_board(repo, game_id)?;
    let mut bombs = load_board_bombs(repo, &board)?;
    let mut players = load_board_players(repo, &board)?;
    let mut explosions = load_board_explosions(repo, &board)?;

    // Tick all bombs
    let bombs_ticked = bombs.len();
    for bomb in &mut bombs {
        bomb.tick()?;
    }

    let tick_num = board.turn + 1;

    // Create tick saga
    let mut saga = TickSaga::default();
    saga.start(
        format!("tick:{}:{}", game_id, tick_num),
        game_id.to_string(),
        bombs_ticked,
    )?;

    let mut explosion_counter = board.explosions_created;
    let mut kill_attributions = Vec::new();

    // ── Phase A: Expand existing explosions ──
    for explosion in &mut explosions {
        if explosion.is_fully_expanded() {
            // Fully expanded last tick → dissipate
            saga.record_dissipation(explosion.entity.id().to_string())?;
            explosion.dissipate()?;
        } else {
            // Expand to next ring
            explosion.expand()?;

            let new_cells = explosion.newly_reached_cells().to_vec();
            let damage = apply_damage(&new_cells, &mut map, &mut players, &mut bombs, None)?;
            record_kill_attributions(
                &mut kill_attributions,
                &damage.players_killed,
                &explosion.bomb_id,
                &explosion.owner,
            );

            if damage.has_damage() {
                saga.record_damage(
                    damage.blocks_destroyed,
                    damage.players_killed,
                    damage.chain_detonations,
                )?;
            }
        }
    }

    // ── Phase B: Detonate ready bombs (iterative for chains) ──
    let mut any_exploded = true;
    while any_exploded {
        any_exploded = false;

        let ready: Vec<usize> = (0..bombs.len())
            .filter(|&i| bombs[i].is_ready_to_explode())
            .collect();

        for idx in ready {
            bombs[idx].explode()?;
            any_exploded = true;

            let rings = calculate_blast_rings(&bombs[idx], &map);
            let bomb_id = bombs[idx].entity.id().to_string();
            let bomb_owner = bombs[idx].owner_id.clone();

            explosion_counter += 1;
            let explosion_id = format!("explosion:{}:{}", game_id, explosion_counter);

            // Create explosion aggregate — center cell (ring 0) is immediately active
            let mut explosion = Explosion::default();
            explosion.start(
                explosion_id.clone(),
                bomb_id.clone(),
                bomb_owner.clone(),
                (bombs[idx].x, bombs[idx].y),
                bombs[idx].blast_radius,
                rings,
            )?;

            // Apply center-cell damage (ring 0)
            let center_cells = explosion.newly_reached_cells().to_vec();
            let damage =
                apply_damage(&center_cells, &mut map, &mut players, &mut bombs, Some(idx))?;
            record_kill_attributions(
                &mut kill_attributions,
                &damage.players_killed,
                &bomb_id,
                &bomb_owner,
            );

            saga.record_detonation(Detonation {
                bomb_id,
                owner: bomb_owner,
                explosion_id,
            })?;

            if damage.has_damage() {
                saga.record_damage(
                    damage.blocks_destroyed,
                    damage.players_killed,
                    damage.chain_detonations,
                )?;
            }

            explosions.push(explosion);
        }
    }

    // ── Phase C: Finalize ──

    // Return bombs to owners
    for bomb in &bombs {
        if bomb.exploded {
            if let Some(player) = players
                .iter_mut()
                .find(|p| p.entity.id() == format!("player:{}", bomb.owner_id))
            {
                player.return_bomb()?;
            }
        }
    }

    // Check win condition
    let alive_players: Vec<&Player> = players.iter().filter(|p| p.alive).collect();
    let game_over = alive_players.len() <= 1 && players.len() > 1;
    let winner = if game_over {
        alive_players.first().map(|p| p.name.clone())
    } else {
        None
    };

    saga.complete(game_over, winner)?;

    // Build board view
    let player_refs: Vec<&Player> = players.iter().collect();
    let bomb_refs: Vec<&Bomb> = bombs.iter().collect();
    let explosion_refs: Vec<&Explosion> = explosions.iter().collect();
    let board = build_board(
        game_id,
        &map,
        &player_refs,
        &bomb_refs,
        &explosion_refs,
        tick_num,
        explosion_counter,
    );

    // Create outbox messages for killed players
    let mut builder = repo.readmodel(&board);
    for killed_id in &saga.players_killed {
        let attribution = kill_attributions
            .iter()
            .find(|attribution| attribution.player_id.as_str() == killed_id.as_str());
        let outbox = OutboxMessage::create(
            format!("player-killed:{}", killed_id),
            "PlayerKilled",
            serde_json::to_vec(&serde_json::json!({
                "player_id": killed_id,
                "killed_by_bomb": attribution
                    .map(|attribution| attribution.bomb_id.as_str())
                    .unwrap_or("unknown"),
                "bomb_owner": attribution
                    .map(|attribution| attribution.bomb_owner.as_str())
                    .unwrap_or("unknown"),
            }))
            .map_err(|err| {
                GameError::Repository(RepositoryError::Model(format!(
                    "outbox payload serialize: {err}"
                )))
            })?,
        )?;
        builder = builder.outbox(outbox);
    }

    // Commit everything atomically
    let mut entities: Vec<&mut sourced_rust::Entity> = Vec::new();
    entities.push(map.entity_mut());
    for player in &mut players {
        entities.push(player.entity_mut());
    }
    for bomb in &mut bombs {
        entities.push(bomb.entity_mut());
    }
    for explosion in &mut explosions {
        entities.push(explosion.entity_mut());
    }
    entities.push(saga.entity_mut());
    builder.commit_many(&mut entities)?;

    Ok(saga)
}

/// Apply damage to cells: destroy blocks, kill players, mark chain detonations.
fn apply_damage(
    cells: &[(i32, i32)],
    map: &mut GameMap,
    players: &mut [Player],
    bombs: &mut [Bomb],
    skip_bomb_idx: Option<usize>,
) -> Result<DamageReport, GameError> {
    let mut report = DamageReport::default();

    for &(cx, cy) in cells {
        // Destroy blocks
        if map.is_in_bounds(cx, cy) && *map.tile_at(cx, cy) == Tile::Block {
            map.destroy_block(cx, cy)?;
            report.blocks_destroyed.push((cx, cy));
        }

        // Kill players
        for player in players.iter_mut() {
            if player.alive && player.x == cx && player.y == cy {
                player.kill()?;
                report.players_killed.push(player.entity.id().to_string());
            }
        }

        // Chain-detonate other bombs
        for (i, bomb) in bombs.iter_mut().enumerate() {
            if Some(i) == skip_bomb_idx {
                continue;
            }
            if !bomb.exploded && bomb.x == cx && bomb.y == cy {
                bomb.ticks_remaining = 0;
                report.chain_detonations.push(bomb.entity.id().to_string());
            }
        }
    }

    Ok(report)
}

// ── Player commands ──

pub fn join_game<R: Commit + ReadModelStore + TransactionalCommit + Get>(
    repo: &R,
    player_id: &str,
    name: &str,
    game_id: &str,
    spawn_index: usize,
) -> Result<(), GameError> {
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

    let player_refs: Vec<&Player> = all_players.iter().collect();
    let bomb_refs: Vec<&Bomb> = all_bombs.iter().collect();
    let explosion_refs: Vec<&Explosion> = all_explosions.iter().collect();
    let board = build_board(
        game_id,
        &map,
        &player_refs,
        &bomb_refs,
        &explosion_refs,
        current_board.turn,
        current_board.explosions_created,
    );

    repo.readmodel(&board).commit(&mut player)?;

    Ok(())
}

pub fn move_player<R: Commit + ReadModelStore + TransactionalCommit + Get>(
    repo: &R,
    player_id: &str,
    direction: Direction,
    game_id: &str,
) -> Result<(), GameError> {
    let mut map: GameMap = repo
        .get_aggregate(game_id)?
        .ok_or(GameError::GameNotFound)?;

    let mut player: Player = repo
        .get_aggregate(&format!("player:{}", player_id))?
        .ok_or(GameError::PlayerNotFound(player_id.to_string()))?;

    if !player.alive {
        return Err(GameError::PlayerDead(player_id.to_string()));
    }

    let (nx, ny) = direction.apply(player.x, player.y);

    if !map.is_in_bounds(nx, ny) {
        return Err(GameError::OutOfBounds(nx, ny));
    }
    if !map.is_passable(nx, ny) {
        return Err(GameError::NotPassable(nx, ny));
    }

    player.move_to(nx, ny)?;

    // Check for power-up collection
    let power_up = map.collect_power_up(nx, ny)?;
    if let Some(pu) = power_up {
        player.apply_power_up(pu)?;
    }

    let current_board = load_board(repo, game_id)?;
    let all_players: Vec<Player> = load_board_players(repo, &current_board)?
        .into_iter()
        .filter(|existing| existing.entity.id() != player.entity.id())
        .collect();
    let mut player_refs: Vec<&Player> = all_players.iter().collect();
    player_refs.push(&player);

    let all_bombs = load_board_bombs(repo, &current_board)?;
    let all_explosions = load_board_explosions(repo, &current_board)?;
    let bomb_refs: Vec<&Bomb> = all_bombs.iter().collect();
    let explosion_refs: Vec<&Explosion> = all_explosions.iter().collect();
    let board = build_board(
        game_id,
        &map,
        &player_refs,
        &bomb_refs,
        &explosion_refs,
        current_board.turn,
        current_board.explosions_created,
    );

    // Commit map (may have power-up collected) and player
    repo.readmodel(&board)
        .commit_many(&mut [map.entity_mut(), player.entity_mut()])?;

    Ok(())
}

pub fn place_bomb<R: Commit + ReadModelStore + TransactionalCommit + Get>(
    repo: &R,
    player_id: &str,
    game_id: &str,
) -> Result<(), GameError> {
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
    let all_players: Vec<Player> = load_board_players(repo, &current_board)?
        .into_iter()
        .filter(|existing| existing.entity.id() != player.entity.id())
        .collect();
    let mut player_refs: Vec<&Player> = all_players.iter().collect();
    player_refs.push(&player);

    let mut all_bombs: Vec<Bomb> = load_board_bombs(repo, &current_board)?
        .into_iter()
        .filter(|existing| existing.entity.id() != bomb.entity.id())
        .collect();
    all_bombs.push(bomb.clone());
    let all_explosions = load_board_explosions(repo, &current_board)?;
    let bomb_refs: Vec<&Bomb> = all_bombs.iter().collect();
    let explosion_refs: Vec<&Explosion> = all_explosions.iter().collect();
    let board = build_board(
        game_id,
        &map,
        &player_refs,
        &bomb_refs,
        &explosion_refs,
        current_board.turn,
        current_board.explosions_created,
    );

    repo.readmodel(&board)
        .commit_many(&mut [player.entity_mut(), bomb.entity_mut()])?;

    Ok(())
}

// ── Internal helpers ──

/// Calculate blast cells organized by ring (distance from center).
/// Ring 0 = center, Ring 1 = cells at distance 1, etc.
/// Walls stop expansion; blocks are included in their ring then stop.
pub fn calculate_blast_rings(bomb: &Bomb, map: &GameMap) -> Vec<Vec<(i32, i32)>> {
    let radius = bomb.blast_radius as usize;
    let mut rings: Vec<Vec<(i32, i32)>> = vec![Vec::new(); radius + 1];

    // Ring 0: center
    rings[0].push((bomb.x, bomb.y));

    let directions = [
        Direction::North,
        Direction::South,
        Direction::East,
        Direction::West,
    ];

    for dir in &directions {
        let mut cx = bomb.x;
        let mut cy = bomb.y;

        for ring in rings.iter_mut().take(radius + 1).skip(1) {
            let (nx, ny) = dir.apply(cx, cy);

            if !map.is_in_bounds(nx, ny) {
                break;
            }

            match map.tile_at(nx, ny) {
                Tile::Wall => break,
                Tile::Block => {
                    ring.push((nx, ny));
                    break;
                }
                _ => {
                    ring.push((nx, ny));
                }
            }

            cx = nx;
            cy = ny;
        }
    }

    rings
}

/// Get a player aggregate by player_id (without the "player:" prefix).
pub fn get_player<R: Get>(repo: &R, player_id: &str) -> Result<Player, GameError> {
    repo.get_aggregate(&format!("player:{}", player_id))?
        .ok_or(GameError::PlayerNotFound(player_id.to_string()))
}
