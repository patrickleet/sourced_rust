//! Pure post-move board snapshot.

use super::tile;
use super::Direction;

/// Pure post-move board snapshot (no ownership / aggregate checks).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MovePreview {
    pub map: Vec<Vec<u8>>,
    pub score: i64,
    pub player_dead: bool,
    pub level_complete: bool,
}

impl MovePreview {
    pub fn status(&self) -> String {
        if self.player_dead {
            "dead".into()
        } else if self.level_complete {
            "level_complete".into()
        } else {
            "active".into()
        }
    }
}

/// Failures from pure board simulation (fail-closed on the client).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SimulateError {
    NoActiveLevel,
    CannotMove(&'static str),
}

/// Apply one direction to a map + score.
pub fn simulate_move(
    map: &[Vec<u8>],
    score: i64,
    direction: Direction,
) -> Result<MovePreview, SimulateError> {
    if map.is_empty() || map[0].is_empty() {
        return Err(SimulateError::NoActiveLevel);
    }
    let (r, c) = player_pos_in(map)?;
    let (nr, nc) = match direction {
        Direction::Up => {
            if r == 0 {
                return Err(SimulateError::CannotMove("row already 0"));
            }
            (r - 1, c)
        }
        Direction::Down => {
            if r + 1 >= map.len() {
                return Err(SimulateError::CannotMove("already at bottom edge"));
            }
            (r + 1, c)
        }
        Direction::Left => {
            if c == 0 {
                return Err(SimulateError::CannotMove("column already 0"));
            }
            (r, c - 1)
        }
        Direction::Right => {
            if c + 1 >= map[r].len() {
                return Err(SimulateError::CannotMove("already at right edge"));
            }
            (r, c + 1)
        }
    };

    let mut next_map = map.to_vec();
    let mut score = score;
    let mut player_dead = false;
    let mut level_complete = false;

    next_map[r][c] = tile::VISITED;
    match next_map[nr][nc] {
        tile::HOLE => next_map[nr][nc] = tile::DEAD_BY_HOLE,
        tile::VISITED => next_map[nr][nc] = tile::DEAD_BY_SUICIDE,
        tile::UNVISITED | tile::PLAYER => {
            score += 1;
            next_map[nr][nc] = tile::PLAYER;
        }
        _ => next_map[nr][nc] = tile::DEAD_BY_SUICIDE,
    }
    for row in &next_map {
        if row.contains(&tile::DEAD_BY_HOLE) || row.contains(&tile::DEAD_BY_SUICIDE) {
            player_dead = true;
            level_complete = false;
            break;
        }
    }
    if !player_dead {
        let any_u = next_map.iter().any(|row| row.contains(&tile::UNVISITED));
        level_complete = !any_u;
    }

    Ok(MovePreview {
        map: next_map,
        score,
        player_dead,
        level_complete,
    })
}

fn player_pos_in(map: &[Vec<u8>]) -> Result<(usize, usize), SimulateError> {
    for (r, row) in map.iter().enumerate() {
        for (c, &t) in row.iter().enumerate() {
            if t == tile::PLAYER {
                return Ok((r, c));
            }
        }
    }
    Err(SimulateError::NoActiveLevel)
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::tile::*;

    fn tiny() -> Vec<Vec<u8>> {
        vec![
            vec![PLAYER, UNVISITED, UNVISITED],
            vec![UNVISITED, UNVISITED, UNVISITED],
            vec![UNVISITED, UNVISITED, UNVISITED],
        ]
    }

    #[test]
    fn move_right_increments_score() {
        let preview = simulate_move(&tiny(), 0, Direction::Right).unwrap();
        assert_eq!(preview.score, 1);
        assert!(!preview.player_dead);
        assert_eq!(preview.map[0][0], VISITED);
        assert_eq!(preview.map[0][1], PLAYER);
    }

    #[test]
    fn edge_fails_closed() {
        assert!(matches!(
            simulate_move(&tiny(), 0, Direction::Up),
            Err(SimulateError::CannotMove(_))
        ));
    }
}
