use serde::{Deserialize, Serialize};
use sourced_rust::{digest, Entity};

/// Tile states for the game grid
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum TileState {
    Hole = 0,
    Unvisited = 1,
    Visited = 2,
    Player = 9,
    DeadByHole = 10,
    DeadBySuicide = 11,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Coordinate {
    pub column: usize,
    pub row: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Level {
    pub id: u32,
    pub map: Vec<Vec<TileState>>,
    pub completed: bool,
}

pub struct BlobGame {
    pub entity: Entity,
    address: String,
    current_level: u32,
    levels: Vec<Level>,
    minigame_id: String,
    player_dead: bool,
    current_level_completed: bool,
    score: u32,
    timed: bool,
    started_at: Option<u64>, // timestamp ms
    last_move_time: Option<u64>,
}

impl Default for BlobGame {
    fn default() -> Self {
        Self {
            entity: Entity::default(),
            address: String::new(),
            current_level: 0,
            levels: Vec::new(),
            minigame_id: String::new(),
            player_dead: false,
            // Level 0 is "completed", so start with current_level_completed as true
            current_level_completed: true,
            score: 0,
            timed: false,
            started_at: None,
            last_move_time: None,
        }
    }
}

const FIVE_MINUTES_MS: u64 = 5 * 60 * 1000;
const FIVE_SECONDS_MS: u64 = 5 * 1000;
const TIME_TO_END: u64 = FIVE_MINUTES_MS + FIVE_SECONDS_MS;

#[allow(dead_code)]
impl BlobGame {
    pub fn new() -> Self {
        Self::default()
    }

    // Getters (don't modify state)
    pub fn current_level(&self) -> Option<&Level> {
        if self.current_level > 0 {
            self.levels.get((self.current_level - 1) as usize)
        } else {
            None
        }
    }

    fn current_level_mut(&mut self) -> Option<&mut Level> {
        if self.current_level > 0 {
            self.levels.get_mut((self.current_level - 1) as usize)
        } else {
            None
        }
    }

    fn player_position(&self) -> Option<Coordinate> {
        let level = self.current_level()?;
        for (row_idx, row) in level.map.iter().enumerate() {
            for (col_idx, &tile) in row.iter().enumerate() {
                if tile == TileState::Player {
                    return Some(Coordinate {
                        column: col_idx,
                        row: row_idx,
                    });
                }
            }
        }
        None
    }

    pub fn score(&self) -> u32 {
        self.score
    }

    pub fn is_player_dead(&self) -> bool {
        self.player_dead
    }

    pub fn is_current_level_completed(&self) -> bool {
        self.current_level_completed
    }

    pub fn address(&self) -> &str {
        &self.address
    }

    pub fn minigame_id(&self) -> &str {
        &self.minigame_id
    }

    // Private helpers
    fn execute_move(&mut self, from: Coordinate, to: Coordinate, move_time: Option<u64>) {
        let mut skip_movement = false;

        // Check for timeout in timed games
        if self.timed {
            if let (Some(move_time), Some(started_at)) = (move_time, self.started_at) {
                self.last_move_time = Some(move_time);
                if move_time.saturating_sub(started_at) >= TIME_TO_END {
                    if let Some(level) = self.current_level_mut() {
                        level.map[from.row][from.column] = TileState::DeadBySuicide;
                    }
                    skip_movement = true;
                }
            }
        }

        if !skip_movement {
            let mut should_increment_score = false;
            if let Some(level) = self.current_level_mut() {
                level.map[from.row][from.column] = TileState::Visited;

                let new_tile = level.map[to.row][to.column];
                match new_tile {
                    TileState::Hole => {
                        level.map[to.row][to.column] = TileState::DeadByHole;
                    }
                    TileState::Visited => {
                        level.map[to.row][to.column] = TileState::DeadBySuicide;
                    }
                    _ => {
                        should_increment_score = true;
                        level.map[to.row][to.column] = TileState::Player;
                    }
                }
            }
            if should_increment_score {
                self.score += 1;
            }
        }

        self.evaluate_level_state();
    }

    fn evaluate_level_state(&mut self) {
        if let Some(level) = self.current_level_mut() {
            // Check for death
            for row in &level.map {
                if row.contains(&TileState::DeadByHole) || row.contains(&TileState::DeadBySuicide) {
                    self.player_dead = true;
                    return;
                }
            }

            // Check for win
            let mut all_visited = true;
            for row in &level.map {
                if row.contains(&TileState::Unvisited) {
                    all_visited = false;
                    break;
                }
            }

            if all_visited {
                level.completed = true;
                self.current_level_completed = true;
            }
        }
    }

    // Commands (digest events)
    #[digest("Initialized")]
    pub fn initialize(
        &mut self,
        id: String,
        address: String,
        minigame_id: String,
        timed: bool,
        started_at: Option<u64>,
    ) {
        self.entity.set_id(&id);
        self.address = address;
        self.minigame_id = minigame_id;
        self.timed = timed;
        if timed {
            self.started_at = started_at;
        }
    }

    #[digest("NextLevelStarted", when = self.current_level_completed && !self.player_dead)]
    pub fn start_next_level(&mut self, map: Vec<Vec<TileState>>) {
        self.current_level += 1;
        let level = Level {
            id: self.current_level,
            map,
            completed: false,
        };
        self.levels.push(level);
        self.current_level_completed = false;
    }

    #[digest("MovedUp", when = self.can_move_up())]
    pub fn up(&mut self, time: Option<u64>) {
        if let Some(pos) = self.player_position() {
            let new_pos = Coordinate {
                column: pos.column,
                row: pos.row - 1,
            };
            self.execute_move(pos, new_pos, time);
        }
    }

    #[digest("MovedDown", when = self.can_move_down())]
    pub fn down(&mut self, time: Option<u64>) {
        if let Some(pos) = self.player_position() {
            let new_pos = Coordinate {
                column: pos.column,
                row: pos.row + 1,
            };
            self.execute_move(pos, new_pos, time);
        }
    }

    #[digest("MovedLeft", when = self.can_move_left())]
    pub fn left(&mut self, time: Option<u64>) {
        if let Some(pos) = self.player_position() {
            let new_pos = Coordinate {
                column: pos.column - 1,
                row: pos.row,
            };
            self.execute_move(pos, new_pos, time);
        }
    }

    #[digest("MovedRight", when = self.can_move_right())]
    pub fn right(&mut self, time: Option<u64>) {
        if let Some(pos) = self.player_position() {
            let new_pos = Coordinate {
                column: pos.column + 1,
                row: pos.row,
            };
            self.execute_move(pos, new_pos, time);
        }
    }

    // Movement validation helpers
    fn can_move_up(&self) -> bool {
        !self.player_dead
            && !self.current_level_completed
            && self.player_position().map(|p| p.row > 0).unwrap_or(false)
    }

    fn can_move_down(&self) -> bool {
        if self.player_dead || self.current_level_completed {
            return false;
        }
        if let (Some(pos), Some(level)) = (self.player_position(), self.current_level()) {
            pos.row < level.map.len() - 1
        } else {
            false
        }
    }

    fn can_move_left(&self) -> bool {
        !self.player_dead
            && !self.current_level_completed
            && self
                .player_position()
                .map(|p| p.column > 0)
                .unwrap_or(false)
    }

    fn can_move_right(&self) -> bool {
        if self.player_dead || self.current_level_completed {
            return false;
        }
        if let (Some(pos), Some(level)) = (self.player_position(), self.current_level()) {
            if let Some(row) = level.map.get(pos.row) {
                pos.column < row.len() - 1
            } else {
                false
            }
        } else {
            false
        }
    }

    pub fn snapshot(&self) -> BlobGameSnapshot {
        BlobGameSnapshot {
            id: self.entity.id().to_string(),
            address: self.address.clone(),
            minigame_id: self.minigame_id.clone(),
            current_level: self.current_level,
            levels: self.levels.clone(),
            player_dead: self.player_dead,
            current_level_completed: self.current_level_completed,
            score: self.score,
            timed: self.timed,
            started_at: self.started_at,
        }
    }
}

// For replay, we need to handle the map serialization
sourced_rust::aggregate!(BlobGame, entity {
    "Initialized"(id, address, minigame_id, timed, started_at) => initialize,
    "NextLevelStarted"(map) => start_next_level,
    "MovedUp"(time) => up,
    "MovedDown"(time) => down,
    "MovedLeft"(time) => left,
    "MovedRight"(time) => right,
});

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BlobGameSnapshot {
    pub id: String,
    pub address: String,
    pub minigame_id: String,
    pub current_level: u32,
    pub levels: Vec<Level>,
    pub player_dead: bool,
    pub current_level_completed: bool,
    pub score: u32,
    pub timed: bool,
    pub started_at: Option<u64>,
}
