//! Blob game domain models.

mod blob_error;
mod blob_game;
mod blob_game_state;
mod blob_games;
mod direction;
pub mod tile;

pub use blob_error::BlobError;
pub use blob_game::{
    test_map_no_holes, test_map_with_hole, BlobGame,
    BlobGameInitializedDomainEvent as BlobInitializedDomainEvent,
    BlobGameLevelStartedDomainEvent as BlobLevelStartedDomainEvent,
    BlobGameMovedDomainEvent as BlobMovedDomainEvent,
    BlobGameStartedDomainEvent as BlobStartedDomainEvent,
};
pub use blob_game_state::BlobGameState;
pub use blob_games::BlobGames;
pub use direction::Direction;
