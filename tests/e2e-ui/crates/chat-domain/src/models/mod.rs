//! Chat domain models.

mod chat_error;
mod chat_message;
mod chat_message_state;

pub use chat_error::ChatError;
pub use chat_message::{domain_commands, ChatMessage, ChatMessagePostedDomainEvent};
pub use chat_message_state::ChatMessageState;
