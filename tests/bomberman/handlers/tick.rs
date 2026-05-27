use sourced_rust::{
    Aggregate, Commit, Get, GetAggregate, OutboxMessage, ReadModelWritePlanCommitExt,
    ReadModelWritePlanStore, RelationalReadModelQueryStore, RepositoryError, TransactionalCommit,
};

use super::shared::{
    board_write_plan, build_board_from_aggregates, load_board, load_board_bombs,
    load_board_explosions, load_board_players,
};
use crate::domain::bomb::Bomb;
use crate::domain::explosion::Explosion;
use crate::domain::game_map::GameMap;
use crate::domain::player::Player;
use crate::domain::tick_saga::{Detonation, TickSaga};
use crate::domain::types::{Direction, Tile};
use crate::error::GameError;

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

pub fn tick<R>(repo: &R, game_id: &str) -> Result<TickSaga, GameError>
where
    R: Commit + TransactionalCommit + Get + ReadModelWritePlanStore + RelationalReadModelQueryStore,
{
    let mut map: GameMap = repo
        .get_aggregate(game_id)?
        .ok_or(GameError::GameNotFound)?;

    let board = load_board(repo, game_id)?;
    let mut bombs = load_board_bombs(repo, &board)?;
    let mut players = load_board_players(repo, &board)?;
    let mut explosions = load_board_explosions(repo, &board)?;

    let bombs_ticked = bombs.len();
    for bomb in &mut bombs {
        bomb.tick()?;
    }

    let tick_num = board.turn + 1;

    let mut saga = TickSaga::default();
    saga.start(
        format!("tick:{}:{}", game_id, tick_num),
        game_id.to_string(),
        bombs_ticked,
    )?;

    let mut explosion_counter = board.explosions_created;
    let mut kill_attributions = Vec::new();

    for explosion in &mut explosions {
        if explosion.is_fully_expanded() {
            saga.record_dissipation(explosion.entity.id().to_string())?;
            explosion.dissipate()?;
        } else {
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

            let mut explosion = Explosion::default();
            explosion.start(
                explosion_id.clone(),
                bomb_id.clone(),
                bomb_owner.clone(),
                (bombs[idx].x, bombs[idx].y),
                bombs[idx].blast_radius,
                rings,
            )?;

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

    let alive_players: Vec<&Player> = players.iter().filter(|p| p.alive).collect();
    let game_over = alive_players.len() <= 1 && players.len() > 1;
    let winner = if game_over {
        alive_players.first().map(|p| p.name.clone())
    } else {
        None
    };

    saga.complete(game_over, winner)?;

    let board = build_board_from_aggregates(
        game_id,
        &map,
        &players,
        &bombs,
        &explosions,
        tick_num,
        explosion_counter,
    );

    let mut builder = repo.read_models(board_write_plan(&board)?);
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

fn apply_damage(
    cells: &[(i32, i32)],
    map: &mut GameMap,
    players: &mut [Player],
    bombs: &mut [Bomb],
    skip_bomb_idx: Option<usize>,
) -> Result<DamageReport, GameError> {
    let mut report = DamageReport::default();

    for &(cx, cy) in cells {
        if map.is_in_bounds(cx, cy) && *map.tile_at(cx, cy) == Tile::Block {
            map.destroy_block(cx, cy)?;
            report.blocks_destroyed.push((cx, cy));
        }

        for player in players.iter_mut() {
            if player.alive && player.x == cx && player.y == cy {
                player.kill()?;
                report.players_killed.push(player.entity.id().to_string());
            }
        }

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

fn calculate_blast_rings(bomb: &Bomb, map: &GameMap) -> Vec<Vec<(i32, i32)>> {
    let radius = bomb.blast_radius as usize;
    let mut rings: Vec<Vec<(i32, i32)>> = vec![Vec::new(); radius + 1];

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
                _ => ring.push((nx, ny)),
            }

            cx = nx;
            cy = ny;
        }
    }

    rings
}
