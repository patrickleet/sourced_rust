//! Todo service crate for the celld example.
//!
//! Domain commands stay in `todo-domain`. Wait-dispatch create/complete to
//! celld; drain cell outbox onto NATS for SQL lists.

mod bounds;
mod handlers;
mod host;
mod routes;

pub use host::CelldTodoCommandHost;
pub use routes::{routes, MODULE_ID};
