use sourced_rust::{digest, Entity};

use super::types::{PowerUp, Tile};

#[derive(Default)]
pub struct GameMap {
    pub entity: Entity,
    pub width: usize,
    pub height: usize,
    pub tiles: Vec<Vec<Tile>>,
    pub spawn_points: Vec<(i32, i32)>,
    pub power_ups: Vec<((i32, i32), PowerUp)>,
}

impl GameMap {
    #[digest("MapCreated")]
    pub fn create(
        &mut self,
        id: String,
        width: usize,
        height: usize,
        tiles: Vec<Vec<Tile>>,
        spawn_points: Vec<(i32, i32)>,
    ) {
        self.entity.set_id(&id);
        self.width = width;
        self.height = height;
        self.tiles = tiles;
        self.spawn_points = spawn_points;
        self.power_ups = vec![];
    }

    #[digest("BlockDestroyed", when = self.tile_at(x, y) == &Tile::Block)]
    pub fn destroy_block(&mut self, x: i32, y: i32) {
        self.tiles[y as usize][x as usize] = Tile::Floor;
        // 50% chance to reveal a power-up based on position parity
        if (x + y) % 2 == 0 {
            self.power_ups.push(((x, y), PowerUp::BombUp));
        } else {
            self.power_ups.push(((x, y), PowerUp::FireUp));
        }
    }

    #[digest("PowerUpCollected")]
    pub fn collect_power_up(&mut self, x: i32, y: i32) -> Option<PowerUp> {
        if let Some(idx) = self.power_ups.iter().position(|((px, py), _)| *px == x && *py == y) {
            let (_, power_up) = self.power_ups.remove(idx);
            Some(power_up)
        } else {
            None
        }
    }

    pub fn is_passable(&self, x: i32, y: i32) -> bool {
        if !self.is_in_bounds(x, y) {
            return false;
        }
        matches!(self.tiles[y as usize][x as usize], Tile::Floor | Tile::Spawn)
    }

    pub fn is_in_bounds(&self, x: i32, y: i32) -> bool {
        x >= 0 && y >= 0 && (x as usize) < self.width && (y as usize) < self.height
    }

    pub fn tile_at(&self, x: i32, y: i32) -> &Tile {
        &self.tiles[y as usize][x as usize]
    }

    /// Parse an ASCII map into tiles and spawn points.
    ///
    /// Legend:
    /// - `#` = Wall
    /// - `.` = Block (destructible)
    /// - ` ` = Floor
    /// - `1`-`4` = Spawn points (stored as Floor tiles)
    pub fn from_ascii(ascii: &str) -> (usize, usize, Vec<Vec<Tile>>, Vec<(i32, i32)>) {
        let lines: Vec<&str> = ascii.lines().filter(|l| !l.is_empty()).collect();
        let height = lines.len();
        let width = lines.iter().map(|l| l.len()).max().unwrap_or(0);

        let mut tiles = Vec::with_capacity(height);
        let mut spawn_points: Vec<Option<(i32, i32)>> = vec![None; 4];

        for (y, line) in lines.iter().enumerate() {
            let mut row = Vec::with_capacity(width);
            for (x, ch) in line.chars().enumerate() {
                match ch {
                    '#' => row.push(Tile::Wall),
                    '.' => row.push(Tile::Block),
                    '1' => {
                        row.push(Tile::Spawn);
                        spawn_points[0] = Some((x as i32, y as i32));
                    }
                    '2' => {
                        row.push(Tile::Spawn);
                        spawn_points[1] = Some((x as i32, y as i32));
                    }
                    '3' => {
                        row.push(Tile::Spawn);
                        spawn_points[2] = Some((x as i32, y as i32));
                    }
                    '4' => {
                        row.push(Tile::Spawn);
                        spawn_points[3] = Some((x as i32, y as i32));
                    }
                    _ => row.push(Tile::Floor),
                }
            }
            // Pad row to width
            while row.len() < width {
                row.push(Tile::Floor);
            }
            tiles.push(row);
        }

        let spawns: Vec<(i32, i32)> = spawn_points.into_iter().flatten().collect();
        (width, height, tiles, spawns)
    }
}

sourced_rust::aggregate!(GameMap, entity {
    "MapCreated"(id, width, height, tiles, spawn_points) => create,
    "BlockDestroyed"(x, y) => destroy_block,
    "PowerUpCollected"(x, y) => collect_power_up,
});
