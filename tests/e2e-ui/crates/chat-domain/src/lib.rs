//! Chat message aggregate — post to a room; author is the session user.

pub mod models;
pub mod projection;

pub use models::{
    ChatError, ChatMessage, ChatMessagePostedDomainEvent, ChatMessageState, ChatMessages,
};
pub use projection::CHAT_MESSAGES;
