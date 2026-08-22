//! Chat message aggregate — post to a room; author is the session user.

pub mod commands;
pub mod models;

pub use commands::{handle_post, post, ChatPostInput, ChatPostPayload, Post};
pub use models::{
    domain_commands, ChatError, ChatMessage, ChatMessagePostedDomainEvent, ChatMessageState,
};
