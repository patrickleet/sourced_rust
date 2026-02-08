use serde::{Deserialize, Serialize};
use sourced_rust::ReadModel;

use crate::domain::types::Tile;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlayerState {
    pub id: String,
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub alive: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BombState {
    pub id: String,
    pub owner: String,
    pub x: i32,
    pub y: i32,
    pub ticks: u8,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExplosionState {
    pub id: String,
    pub owner: String,
    pub cells: Vec<(i32, i32)>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ReadModel)]
#[readmodel(collection = "boards")]
pub struct BoardView {
    #[readmodel(id)]
    pub game_id: String,
    pub width: usize,
    pub height: usize,
    pub tiles: Vec<Vec<Tile>>,
    pub players: Vec<PlayerState>,
    pub bombs: Vec<BombState>,
    pub explosions: Vec<ExplosionState>,
    pub turn: u32,
}

impl BoardView {
    pub fn new(game_id: &str, width: usize, height: usize, tiles: Vec<Vec<Tile>>) -> Self {
        Self {
            game_id: game_id.to_string(),
            width,
            height,
            tiles,
            players: vec![],
            bombs: vec![],
            explosions: vec![],
            turn: 0,
        }
    }
}

pub fn build_board(
    game_id: &str,
    map: &crate::domain::game_map::GameMap,
    players: &[&crate::domain::player::Player],
    bombs: &[&crate::domain::bomb::Bomb],
    explosions: &[&crate::domain::explosion::Explosion],
    turn: u32,
) -> BoardView {
    let mut board = BoardView::new(game_id, map.width, map.height, map.tiles.clone());
    board.turn = turn;

    for p in players {
        board.players.push(PlayerState {
            id: p.entity.id().to_string(),
            name: p.name.clone(),
            x: p.x,
            y: p.y,
            alive: p.alive,
        });
    }

    for b in bombs {
        if !b.exploded {
            board.bombs.push(BombState {
                id: b.entity.id().to_string(),
                owner: b.owner_id.clone(),
                x: b.x,
                y: b.y,
                ticks: b.ticks_remaining,
            });
        }
    }

    for e in explosions {
        if e.active {
            board.explosions.push(ExplosionState {
                id: e.entity.id().to_string(),
                owner: e.owner.clone(),
                cells: e.all_active_cells(),
            });
        }
    }

    board
}
