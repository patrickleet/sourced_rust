//! Blob game domain models.

mod blob_error;
mod blob_game;
mod blob_game_fact;
mod direction;
pub mod tile;

pub use blob_error::BlobError;
pub use blob_game::{test_map_no_holes, test_map_with_hole, BlobGame};
pub use blob_game_fact::BlobGameFact;
pub use direction::Direction;
