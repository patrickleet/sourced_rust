//! Procedural levels inspired by ig-blob-game-model-service `lib/levels.ts`.
//!
//! Holes are placed with spacing heuristics (like the original), then we **prove
//! passability**: there exists a path that visits every non-hole cell exactly once
//! starting from the player (Hamiltonian path on the free-cell graph). Small grids
//! make that check feasible.

use rand::seq::SliceRandom;
use rand::{Rng, SeedableRng};
use rand::rngs::StdRng;

use crate::models::tile;

/// Playfield size. Original is 12×12; we use 6×6 so Hamiltonian passability
/// checks stay fast on the command path while still varying layouts.
pub const DEFAULT_SIZE: usize = 6;
/// Holes on level 1 (scales up with level index, capped).
pub const DEFAULT_HOLES: usize = 2;

/// Fixed map used only in unit tests that need determinism without RNG.
pub fn demo_map() -> Vec<Vec<u8>> {
    use tile::*;
    vec![
        vec![PLAYER, UNVISITED, UNVISITED, UNVISITED, HOLE],
        vec![UNVISITED, UNVISITED, UNVISITED, UNVISITED, UNVISITED],
        vec![UNVISITED, UNVISITED, HOLE, UNVISITED, UNVISITED],
        vec![UNVISITED, UNVISITED, UNVISITED, UNVISITED, UNVISITED],
        vec![UNVISITED, UNVISITED, UNVISITED, UNVISITED, UNVISITED],
    ]
}

/// Generate a random, **solvable** level. Retries until passable or falls back
/// to a dense no-hole grid (always solvable).
pub fn generate_level(level_index: u32) -> Vec<Vec<u8>> {
    let mut rng = StdRng::from_entropy();
    generate_level_with(&mut rng, level_index)
}

/// Same as [`generate_level`] with an injected RNG (tests / seeds).
pub fn generate_level_with<R: Rng>(rng: &mut R, level_index: u32) -> Vec<Vec<u8>> {
    let size = DEFAULT_SIZE;
    // Slightly more holes as levels progress (capped).
    let holes = (DEFAULT_HOLES + level_index.saturating_sub(1) as usize).min(size / 2);

    for _ in 0..80 {
        if let Some(map) = try_generate(rng, size, holes) {
            if is_hamiltonian_passable(&map) {
                return map;
            }
        }
    }

    // Guaranteed solvable: no holes.
    empty_playable(size)
}

fn empty_playable(size: usize) -> Vec<Vec<u8>> {
    let mut map = vec![vec![tile::UNVISITED; size]; size];
    map[0][0] = tile::PLAYER;
    map
}

fn try_generate<R: Rng>(rng: &mut R, size: usize, hole_count: usize) -> Option<Vec<Vec<u8>>> {
    let mut map = empty_playable(size);
    let mut candidates: Vec<(usize, usize)> = Vec::new();
    for r in 0..size {
        for c in 0..size {
            // Keep start clear; avoid corner-trapping start neighbors sometimes
            if (r, c) == (0, 0) {
                continue;
            }
            // Soft perimeter rule from original: fewer holes on the outer ring
            // for the first two rings near start.
            if (r == 0 && c == 1) || (r == 1 && c == 0) {
                continue;
            }
            candidates.push((r, c));
        }
    }
    candidates.shuffle(rng);

    let mut placed: Vec<(usize, usize)> = Vec::new();
    for (r, c) in candidates {
        if placed.len() >= hole_count {
            break;
        }
        if hole_ok(&placed, r, c, size) {
            map[r][c] = tile::HOLE;
            placed.push((r, c));
        }
    }

    // Connectivity of free cells (necessary, not sufficient)
    if !is_connected_free(&map) {
        return None;
    }
    Some(map)
}

/// Original-inspired spacing: holes not too close (Manhattan < 3 on small grid).
fn hole_ok(placed: &[(usize, usize)], r: usize, c: usize, size: usize) -> bool {
    // Don't block the entire first row/col start corridor
    if r == 0 && c < 2 {
        return false;
    }
    if c == 0 && r < 2 {
        return false;
    }
    // Prefer not on absolute outer edge for tiny grids (original avoided perimeter)
    if size >= 6 && (r == size - 1 || c == size - 1) && (r == 0 || c == 0) {
        // allow but continue checks
    }
    for &(pr, pc) in placed {
        let dist = pr.abs_diff(r) + pc.abs_diff(c);
        if dist < 3 {
            return false;
        }
        // Reciprocal / diagonal pattern heuristics (simplified from original)
        if pr == c && pc == r {
            return false;
        }
    }
    true
}

fn free_cells(map: &[Vec<u8>]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for (r, row) in map.iter().enumerate() {
        for (c, &t) in row.iter().enumerate() {
            if t != tile::HOLE {
                out.push((r, c));
            }
        }
    }
    out
}

fn is_connected_free(map: &[Vec<u8>]) -> bool {
    let free = free_cells(map);
    if free.is_empty() {
        return false;
    }
    let h = map.len();
    let w = map[0].len();
    let mut seen = vec![vec![false; w]; h];
    let mut stack = vec![(0usize, 0usize)];
    if map[0][0] == tile::HOLE {
        return false;
    }
    seen[0][0] = true;
    let mut count = 0usize;
    while let Some((r, c)) = stack.pop() {
        count += 1;
        for (nr, nc) in neighbors(r, c, h, w) {
            if map[nr][nc] != tile::HOLE && !seen[nr][nc] {
                seen[nr][nc] = true;
                stack.push((nr, nc));
            }
        }
    }
    count == free.len()
}

fn neighbors(r: usize, c: usize, h: usize, w: usize) -> Vec<(usize, usize)> {
    let mut n = Vec::with_capacity(4);
    if r > 0 {
        n.push((r - 1, c));
    }
    if r + 1 < h {
        n.push((r + 1, c));
    }
    if c > 0 {
        n.push((r, c - 1));
    }
    if c + 1 < w {
        n.push((r, c + 1));
    }
    n
}

/// True if some path visits every non-hole cell exactly once from the player.
pub fn is_hamiltonian_passable(map: &[Vec<u8>]) -> bool {
    let free = free_cells(map);
    let n = free.len();
    if n == 0 {
        return false;
    }
    let h = map.len();
    let w = map[0].len();
    let mut index = vec![vec![None; w]; h];
    for (i, &(r, c)) in free.iter().enumerate() {
        index[r][c] = Some(i);
    }
    // adjacency list
    let mut adj = vec![Vec::new(); n];
    for (i, &(r, c)) in free.iter().enumerate() {
        for (nr, nc) in neighbors(r, c, h, w) {
            if let Some(j) = index[nr][nc] {
                adj[i].push(j);
            }
        }
    }
    let start = index[0][0].expect("player at 0,0");
    let mut visited = vec![false; n];
    visited[start] = true;
    hamilton_dfs(start, 1, n, &adj, &mut visited)
}

fn hamilton_dfs(
    u: usize,
    depth: usize,
    n: usize,
    adj: &[Vec<usize>],
    visited: &mut [bool],
) -> bool {
    if depth == n {
        return true;
    }
    for &v in &adj[u] {
        if !visited[v] {
            visited[v] = true;
            if hamilton_dfs(v, depth + 1, n, adj, visited) {
                return true;
            }
            visited[v] = false;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;

    #[test]
    fn generated_maps_differ_and_are_passable() {
        let mut rng = StdRng::seed_from_u64(42);
        let a = generate_level_with(&mut rng, 1);
        let b = generate_level_with(&mut rng, 1);
        assert!(is_hamiltonian_passable(&a));
        assert!(is_hamiltonian_passable(&b));
        // Same seed stream still progresses — maps should usually differ
        assert_ne!(a, b, "expected variety across sequential generates");
        assert_eq!(a[0][0], tile::PLAYER);
        assert_eq!(b[0][0], tile::PLAYER);
    }

    #[test]
    fn empty_grid_passable() {
        assert!(is_hamiltonian_passable(&empty_playable(5)));
    }

    #[test]
    fn disconnected_not_passable() {
        let mut m = empty_playable(3);
        // Wall of holes isolating bottom-right
        m[0][1] = tile::HOLE;
        m[1][0] = tile::HOLE;
        m[1][1] = tile::HOLE;
        assert!(!is_connected_free(&m) || !is_hamiltonian_passable(&m));
    }
}
