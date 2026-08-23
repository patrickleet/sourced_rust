//! Todo service crate for the celld example.
//!
//! Domain commands stay in `todo-domain`. This crate mounts them for local
//! dual-write (SQL lists) and wait-dispatches `todo.create` / `todo.complete`
//! to celld through [`CelldTodoCommandHost`].

mod bounds;
mod handlers;
mod host;
mod routes;

pub use host::CelldTodoCommandHost;
pub use routes::{routes, MODULE_ID};
