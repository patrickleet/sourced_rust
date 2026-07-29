//! BlobGame aggregate — grid trail game (remake of ig-blob-game-model-service).
//!
//! Tile ints match JS `constants.ts` (player=9, hole=0, unvisited=1, visited=2,
//! dead_by_suicide=3, dead_by_hole=4). Read models update only from emitted facts.

pub mod levels;
pub mod models;

#[doc(hidden)]
pub mod projection_v2;

pub use levels::{demo_map, generate_level, generate_level_with, is_hamiltonian_passable};
pub use models::tile;
pub use models::{
    test_map_no_holes, test_map_with_hole, BlobError, BlobGame, BlobGameFact, Direction,
};
