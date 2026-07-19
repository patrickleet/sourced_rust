use serde::{Deserialize, Serialize};

use super::ChatMessage;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMessagePosted {
    pub message_id: String,
    pub room_id: String,
    pub author_id: String,
    pub body: String,
    pub created_at: String,
}

impl ChatMessagePosted {
    pub fn from_message(m: &ChatMessage) -> Self {
        Self {
            message_id: m.message_id.clone(),
            room_id: m.room_id.clone(),
            author_id: m.author_id.clone(),
            body: m.body.clone(),
            created_at: m.created_at.clone(),
        }
    }
}
