//! Todo service crate for the celld example.
//!
//! Domain commands stay in `todo-domain`. Wait-dispatch create/complete to
//! celld through [`distributed::cell_host::CelldCommandHost`].

mod bounds;
mod handlers;
mod host;
mod routes;

pub use host::celld_route;
pub use routes::{routes, MODULE_ID};
