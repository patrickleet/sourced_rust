//! Chat service crate: lobby messages.
//!
//! SOA mounts stay in-process. The optional celld host wait-dispatches
//! `chat.post` through [`CelldChatCommandHost`] and drains cell outbox onto
//! NATS. GraphQL `@live` stays here.

mod bounds;
mod handlers;
mod host;
mod routes;

pub use host::CelldChatCommandHost;
pub use routes::{routes, MODULE_ID};
