//! Chat message aggregate — post to a room; author is the session user.

use distributed::{sourced, Entity};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ChatError {
    #[error("message already exists")]
    AlreadyExists,
    #[error("empty message id")]
    EmptyId,
    #[error("empty author id")]
    EmptyAuthor,
    #[error("empty room id")]
    EmptyRoom,
    #[error("empty body")]
    EmptyBody,
    #[error(transparent)]
    Event(#[from] distributed::EventRecordError),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChatMessage {
    #[serde(skip, default)]
    pub entity: Entity,
    pub message_id: String,
    pub room_id: String,
    pub author_id: String,
    pub body: String,
    /// RFC3339 timestamp (string for portable projections / SQLite text).
    pub created_at: String,
}

impl ChatMessage {
    pub fn is_posted(&self) -> bool {
        !self.message_id.is_empty()
    }
}

#[sourced(entity, events = "ChatMessageEvent", aggregate_type = "chat_message")]
impl ChatMessage {
    pub fn post(
        &mut self,
        message_id: impl Into<String>,
        room_id: impl Into<String>,
        author_id: impl Into<String>,
        body: impl Into<String>,
        created_at: impl Into<String>,
    ) -> Result<(), ChatError> {
        if self.is_posted() {
            return Err(ChatError::AlreadyExists);
        }
        let message_id = message_id.into();
        let room_id = room_id.into();
        let author_id = author_id.into();
        let body = body.into();
        let created_at = created_at.into();
        if message_id.trim().is_empty() {
            return Err(ChatError::EmptyId);
        }
        if room_id.trim().is_empty() {
            return Err(ChatError::EmptyRoom);
        }
        if author_id.trim().is_empty() {
            return Err(ChatError::EmptyAuthor);
        }
        let body = body.trim();
        if body.is_empty() {
            return Err(ChatError::EmptyBody);
        }
        self.record_posted(
            message_id,
            room_id,
            author_id,
            body.to_string(),
            created_at,
        )?;
        Ok(())
    }

    #[event("chat_message.posted")]
    fn record_posted(
        &mut self,
        message_id: String,
        room_id: String,
        author_id: String,
        body: String,
        created_at: String,
    ) {
        self.entity.set_id(&message_id);
        self.message_id = message_id;
        self.room_id = room_id;
        self.author_id = author_id;
        self.body = body;
        self.created_at = created_at;
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn post_sets_fields() {
        let mut m = ChatMessage::default();
        m.post("m1", "lobby", "alice", "hello", "2026-01-01T00:00:00Z")
            .unwrap();
        assert!(m.is_posted());
        assert_eq!(m.body, "hello");
        assert_eq!(m.author_id, "alice");
    }

    #[test]
    fn rejects_empty_body() {
        let mut m = ChatMessage::default();
        assert_eq!(
            m.post("m1", "lobby", "alice", "  ", "t").unwrap_err(),
            ChatError::EmptyBody
        );
    }
}
