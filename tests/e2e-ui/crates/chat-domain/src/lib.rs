//! Chat message aggregate — post to a room; author is the session user.

pub mod models;

#[doc(hidden)]
pub mod projection_v2;

pub use models::{ChatError, ChatMessage, ChatMessagePosted};
