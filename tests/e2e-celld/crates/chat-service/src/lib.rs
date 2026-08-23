//! Chat service crate: lobby messages (in-process; not a cell).

mod bounds;
mod handlers;
mod routes;

pub use routes::{routes, MODULE_ID};
