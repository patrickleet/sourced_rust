use chat_domain::ChatMessagePosted;
use distributed::ReadModel;
use serde::{Deserialize, Serialize};

use super::AuthUserView;

/// Chat message row. PK: `message_id`. Live via GraphQL subscription on `chat_messages`.
///
/// `author` is a GraphQL join onto imported [`AuthUserView`] (`author_id` = `user_id`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ReadModel)]
#[table("chat_messages")]
pub struct ChatMessageView {
    #[id("message_id")]
    pub message_id: String,
    pub room_id: String,
    pub author_id: String,
    pub body: String,
    pub created_at: String,
    #[readmodel(belongs_to = "AuthUserView", foreign_key = "author_id")]
    pub author: Option<AuthUserView>,
}

pub fn map_chat_posted(e: &ChatMessagePosted) -> ChatMessageView {
    ChatMessageView {
        message_id: e.message_id.clone(),
        room_id: e.room_id.clone(),
        author_id: e.author_id.clone(),
        body: e.body.clone(),
        created_at: e.created_at.clone(),
        author: None,
    }
}
