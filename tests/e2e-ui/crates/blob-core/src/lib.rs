//! Pure blob board rules shared by the domain aggregate and client WASM.
//!
//! No I/O, no `distributed`, no ownership — only map + score + direction.

mod direction;
mod simulate;
pub mod tile;

pub use direction::Direction;
pub use simulate::{simulate_move, MovePreview, SimulateError};
