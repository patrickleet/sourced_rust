use sourced_rust::read_model::ReadModelStore;
use sourced_rust::{
    Aggregate, Commit, CommitBuilderExt, Find, Get, GetAggregate, OutboxMessage, hydrate,
};

use crate::domain::bomb::Bomb;
use crate::domain::explosion::Explosion;
use crate::domain::game_map::GameMap;
use crate::domain::player::Player;
use crate::domain::tick_saga::{Detonation, TickSaga};
use crate::domain::types::{Direction, Tile};
use crate::error::GameError;
use crate::views::{build_board, BoardView};

/// Find aggregates whose entity ID starts with a given prefix, then hydrate and filter.
fn find_by_prefix<R: Find, A: Aggregate, F>(
    repo: &R,
    prefix: &str,
    predicate: F,
) -> Result<Vec<A>, GameError>
where
    F: Fn(&A) -> bool,
{
    let prefix_owned = prefix.to_string();
    let entities = repo.find(|e| e.id().starts_with(&prefix_owned))?;
    let mut results = Vec::new();
    for entity in entities {
        let agg: A = hydrate(entity)?;
        if predicate(&agg) {
            results.push(agg);
        }
    }
    Ok(results)
}

// ── Game commands ──

pub fn create_game<R: Commit + ReadModelStore>(
    repo: &R,
    game_id: &str,
    ascii_map: &str,
) -> Result<GameMap, GameError> {
    let (width, height, tiles, spawn_points) = GameMap::from_ascii(ascii_map);

    let mut map = GameMap::default();
    map.create(
        game_id.into(),
        width,
        height,
        tiles.clone(),
        spawn_points,
    );

    let board = BoardView::new(game_id, width, height, tiles);
    repo.readmodel(&board).commit(&mut map)?;

    Ok(map)
}

pub fn tick<R: Commit + ReadModelStore + Get + sourced_rust::Find>(
    repo: &R,
    game_id: &str,
) -> Result<TickSaga, GameError> {
    let mut map: GameMap = repo
        .get_aggregate(game_id)?
        .ok_or(GameError::GameNotFound)?;

    // Find all active bombs
    let mut bombs: Vec<Bomb> = find_by_prefix(repo, "bomb:", |b: &Bomb| !b.exploded)?;

    // Find all players
    let mut players: Vec<Player> = find_by_prefix(repo, "player:", |_: &Player| true)?;

    // Find all active explosions
    let mut explosions: Vec<Explosion> =
        find_by_prefix(repo, "explosion:", |e: &Explosion| e.active)?;

    // Tick all bombs
    let bombs_ticked = bombs.len();
    for bomb in &mut bombs {
        bomb.tick();
    }

    // Count existing tick sagas to determine saga number
    let existing_ticks: Vec<TickSaga> =
        find_by_prefix(repo, &format!("tick:{}", game_id), |_: &TickSaga| true)?;
    let tick_num = existing_ticks.len() + 1;

    // Create tick saga
    let mut saga = TickSaga::default();
    saga.start(
        format!("tick:{}:{}", game_id, tick_num),
        game_id.to_string(),
        bombs_ticked,
    );

    // Count existing explosions for unique ID generation
    let all_explosions: Vec<Explosion> =
        find_by_prefix(repo, "explosion:", |_: &Explosion| true)?;
    let mut explosion_counter = all_explosions.len();

    // ── Phase A: Expand existing explosions ──
    for explosion in &mut explosions {
        if explosion.is_fully_expanded() {
            // Fully expanded last tick → dissipate
            saga.record_dissipation(explosion.entity.id().to_string());
            explosion.dissipate();
        } else {
            // Expand to next ring
            explosion.expand();

            let new_cells = explosion.newly_reached_cells().to_vec();
            let (blocks, killed, chains) =
                apply_damage(&new_cells, &mut map, &mut players, &mut bombs, None);

            if !blocks.is_empty() || !killed.is_empty() || !chains.is_empty() {
                saga.record_damage(blocks, killed, chains);
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
            bombs[idx].explode();
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
            );

            // Apply center-cell damage (ring 0)
            let center_cells = explosion.newly_reached_cells().to_vec();
            let (blocks, killed, chains) =
                apply_damage(&center_cells, &mut map, &mut players, &mut bombs, Some(idx));

            saga.record_detonation(Detonation {
                bomb_id,
                owner: bomb_owner,
                explosion_id,
            });

            if !blocks.is_empty() || !killed.is_empty() || !chains.is_empty() {
                saga.record_damage(blocks, killed, chains);
            }

            explosions.push(explosion);
        }
    }

    // ── Phase C: Finalize ──

    // Return bombs to owners
    for bomb in &bombs {
        if bomb.exploded {
            if let Some(player) = players.iter_mut().find(|p| {
                p.entity.id() == format!("player:{}", bomb.owner_id)
            }) {
                player.return_bomb();
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

    saga.complete(game_over, winner);

    // Build board view
    let player_refs: Vec<&Player> = players.iter().collect();
    let bomb_refs: Vec<&Bomb> = bombs.iter().collect();
    let explosion_refs: Vec<&Explosion> = explosions.iter().collect();
    let board = build_board(game_id, &map, &player_refs, &bomb_refs, &explosion_refs, 0);

    // Create outbox messages for killed players
    let mut builder = repo.readmodel(&board);
    for killed_id in &saga.players_killed {
        // Find the detonation that caused this kill (use most recent)
        let det = saga.detonations.last();
        let outbox = OutboxMessage::create(
            format!("outbox:killed:{}", killed_id),
            "PlayerKilled",
            serde_json::to_vec(&serde_json::json!({
                "player_id": killed_id,
                "killed_by_bomb": det.map(|d| d.bomb_id.as_str()).unwrap_or("unknown"),
                "bomb_owner": det.map(|d| d.owner.as_str()).unwrap_or("unknown"),
            }))
            .unwrap(),
        );
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
/// Returns (blocks_destroyed, players_killed, chain_detonations).
fn apply_damage(
    cells: &[(i32, i32)],
    map: &mut GameMap,
    players: &mut [Player],
    bombs: &mut [Bomb],
    skip_bomb_idx: Option<usize>,
) -> (Vec<(i32, i32)>, Vec<String>, Vec<String>) {
    let mut blocks_destroyed = Vec::new();
    let mut players_killed = Vec::new();
    let mut chain_detonations = Vec::new();

    for &(cx, cy) in cells {
        // Destroy blocks
        if map.is_in_bounds(cx, cy) && *map.tile_at(cx, cy) == Tile::Block {
            map.destroy_block(cx, cy);
            blocks_destroyed.push((cx, cy));
        }

        // Kill players
        for player in players.iter_mut() {
            if player.alive && player.x == cx && player.y == cy {
                player.kill();
                players_killed.push(player.entity.id().to_string());
            }
        }

        // Chain-detonate other bombs
        for i in 0..bombs.len() {
            if Some(i) == skip_bomb_idx {
                continue;
            }
            if !bombs[i].exploded && bombs[i].x == cx && bombs[i].y == cy {
                bombs[i].ticks_remaining = 0;
                chain_detonations.push(bombs[i].entity.id().to_string());
            }
        }
    }

    (blocks_destroyed, players_killed, chain_detonations)
}

// ── Player commands ──

pub fn join_game<R: Commit + ReadModelStore + Get + sourced_rust::Find>(
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
    player.join(
        format!("player:{}", player_id),
        name.into(),
        sx,
        sy,
    );

    // Find all players for board view
    let mut all_players: Vec<Player> = find_by_prefix(repo, "player:", |_: &Player| true)?;
    all_players.push(player.clone());

    let all_bombs: Vec<Bomb> = find_by_prefix(repo, "bomb:", |b: &Bomb| !b.exploded)?;
    let all_explosions: Vec<Explosion> =
        find_by_prefix(repo, "explosion:", |e: &Explosion| e.active)?;

    let player_refs: Vec<&Player> = all_players.iter().collect();
    let bomb_refs: Vec<&Bomb> = all_bombs.iter().collect();
    let explosion_refs: Vec<&Explosion> = all_explosions.iter().collect();
    let board = build_board(game_id, &map, &player_refs, &bomb_refs, &explosion_refs, 0);

    repo.readmodel(&board).commit(&mut player)?;

    Ok(())
}

pub fn move_player<R: Commit + ReadModelStore + Get + sourced_rust::Find>(
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

    player.move_to(nx, ny);

    // Check for power-up collection
    let power_up = map.collect_power_up(nx, ny);
    if let Some(pu) = power_up {
        player.apply_power_up(pu);
    }

    // Build board view
    let all_players: Vec<Player> = find_by_prefix(repo, "player:", |p: &Player| {
        p.entity.id() != player.entity.id()
    })?;
    let mut player_refs: Vec<&Player> = all_players.iter().collect();
    player_refs.push(&player);

    let all_bombs: Vec<Bomb> = find_by_prefix(repo, "bomb:", |b: &Bomb| !b.exploded)?;
    let all_explosions: Vec<Explosion> =
        find_by_prefix(repo, "explosion:", |e: &Explosion| e.active)?;
    let bomb_refs: Vec<&Bomb> = all_bombs.iter().collect();
    let explosion_refs: Vec<&Explosion> = all_explosions.iter().collect();
    let board = build_board(game_id, &map, &player_refs, &bomb_refs, &explosion_refs, 0);

    // Commit map (may have power-up collected) and player
    repo.readmodel(&board)
        .commit_many(&mut [map.entity_mut(), player.entity_mut()])?;

    Ok(())
}

pub fn place_bomb<R: Commit + ReadModelStore + Get + sourced_rust::Find>(
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

    // Count existing bombs for this player to generate unique ID
    let existing_bombs: Vec<Bomb> = find_by_prefix(repo, "bomb:", |b: &Bomb| {
        b.owner_id == player_id
    })?;
    let bomb_num = existing_bombs.len() + 1;

    player.place_bomb();

    let mut bomb = Bomb::default();
    bomb.create(
        format!("bomb:{}:{}", player_id, bomb_num),
        player_id.into(),
        player.x,
        player.y,
        player.blast_radius,
    );

    // Build board view
    let all_players: Vec<Player> = find_by_prefix(repo, "player:", |p: &Player| {
        p.entity.id() != player.entity.id()
    })?;
    let mut player_refs: Vec<&Player> = all_players.iter().collect();
    player_refs.push(&player);

    let mut all_bombs: Vec<Bomb> = find_by_prefix(repo, "bomb:", |b: &Bomb| {
        !b.exploded && b.entity.id() != bomb.entity.id()
    })?;
    all_bombs.push(bomb.clone());
    let all_explosions: Vec<Explosion> =
        find_by_prefix(repo, "explosion:", |e: &Explosion| e.active)?;
    let bomb_refs: Vec<&Bomb> = all_bombs.iter().collect();
    let explosion_refs: Vec<&Explosion> = all_explosions.iter().collect();
    let board = build_board(game_id, &map, &player_refs, &bomb_refs, &explosion_refs, 0);

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

        for dist in 1..=radius {
            let (nx, ny) = dir.apply(cx, cy);

            if !map.is_in_bounds(nx, ny) {
                break;
            }

            match map.tile_at(nx, ny) {
                Tile::Wall => break,
                Tile::Block => {
                    rings[dist].push((nx, ny));
                    break;
                }
                _ => {
                    rings[dist].push((nx, ny));
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
