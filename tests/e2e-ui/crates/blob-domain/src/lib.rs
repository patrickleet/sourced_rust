//! Blob game domain — one crate, three faces:
//!
//! - [`core`] — pure board rules (always available; WASM-eligible)
//! - [`models`] / [`levels`] — aggregate host (`feature = "domain"`, default)
//! - [`wasm`] — `blobSimulateMove` for the client (`feature = "wasm"`)
//!
//! Tile ints match the client board helpers (player=9, hole=0, unvisited=1, …).

pub mod core;

#[cfg(feature = "domain")]
pub mod commands;
#[cfg(feature = "domain")]
pub mod levels;
#[cfg(feature = "domain")]
pub mod models;

#[cfg(feature = "wasm")]
pub mod wasm;

pub use core::{simulate_move, tile, Direction, MovePreview, SimulateError};

#[cfg(feature = "domain")]
pub use commands::{
    move_dir, start, start_level, BlobMoveInput, BlobStartInput, BlobStartLevelInput, Move, Start,
    StartLevel,
};
#[cfg(feature = "domain")]
pub use levels::{demo_map, generate_level, generate_level_with, is_hamiltonian_passable};
#[cfg(feature = "domain")]
pub use models::{
    domain_commands, test_map_no_holes, test_map_with_hole, BlobError, BlobGame, BlobGameState,
    BlobInitializedDomainEvent, BlobLevelStartedDomainEvent, BlobMovedDomainEvent,
    BlobStartedDomainEvent,
};
