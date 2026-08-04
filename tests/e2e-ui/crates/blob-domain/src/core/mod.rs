//! Pure board rules — no I/O, no `distributed`, WASM-eligible.
//!
//! Shared by the aggregate host (`models`) and the client WASM surface (`wasm`).

mod direction;
mod simulate;
pub mod tile;

pub use direction::Direction;
pub use simulate::{simulate_move, MovePreview, SimulateError};
