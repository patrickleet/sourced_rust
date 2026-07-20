use distributed::{sourced, Entity};
use serde::{Deserialize, Serialize};

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

/// One public command with `#[event]`. Callers pass non-empty trimmed fields.
#[sourced(entity, events = "ChatMessageEvent", aggregate_type = "chat_message")]
impl ChatMessage {
    #[event(
        "chat_message.posted",
        when = !self.is_posted()
            && !message_id.is_empty()
            && !room_id.is_empty()
            && !author_id.is_empty()
            && !body.is_empty()
    )]
    pub fn post(
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn post_sets_fields() {
        let mut m = ChatMessage::default();
        let _ = m.post(
            "m1".into(),
            "lobby".into(),
            "alice".into(),
            "hello".into(),
            "2026-01-01T00:00:00Z".into(),
        );
        assert!(m.is_posted());
        assert_eq!(m.body, "hello");
        assert_eq!(m.author_id, "alice");
    }

    #[test]
    fn empty_body_is_noop() {
        let mut m = ChatMessage::default();
        let _ = m.post(
            "m1".into(),
            "lobby".into(),
            "alice".into(),
            String::new(),
            "t".into(),
        );
        assert!(!m.is_posted());
        assert_eq!(m.entity.version(), 0);
    }
}
