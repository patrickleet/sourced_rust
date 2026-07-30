//! Chat message aggregate — post to a room; author is the session user.

pub mod models;

pub use models::{ChatError, ChatMessage, ChatMessagePostedDomainEvent, ChatMessageState};
