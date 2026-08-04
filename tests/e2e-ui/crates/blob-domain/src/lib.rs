//! BlobGame aggregate — grid trail game (remake of ig-blob-game-model-service).
//!
//! Pure board rules live in [`blob_core`] (WASM-eligible). Tile ints match the
//! client board helpers (player=9, hole=0, unvisited=1, visited=2, …).

pub mod levels;
pub mod models;

pub use levels::{demo_map, generate_level, generate_level_with, is_hamiltonian_passable};
pub use models::tile;
pub use models::{
    domain_commands, simulate_move, test_map_no_holes, test_map_with_hole, BlobError, BlobGame,
    BlobGameState, BlobInitializedDomainEvent, BlobLevelStartedDomainEvent, BlobMovedDomainEvent,
    BlobStartedDomainEvent, Direction, MovePreview,
};
