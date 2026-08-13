//! Compiler-owned projection-delta contracts.
//!
//! `wire` mirrors the frozen Rust response protocol exactly. `preview` is a
//! separate executable artifact IR whose values remain scoped input/preset
//! expressions until the JavaScript command runtime prepares a command.

mod preview;
mod wire;

pub(crate) use preview::{compile_command_preview, CompiledCommandProjection};
pub(crate) use wire::PROJECTION_DELTA_WIRE_VERSION;
