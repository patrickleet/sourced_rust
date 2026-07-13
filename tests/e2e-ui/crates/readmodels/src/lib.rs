//! Read models for the e2e-ui fixture (todos + chat).
//! Projected only from domain events (never from commands).

use chat_domain::ChatMessagePosted;
use distributed::ReadModel;
use serde::{Deserialize, Serialize};
use todo_domain::TodoFact;

/// Personal todo row. PK: `todo_id`. Isolation key: `owner_id`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ReadModel)]
#[table("todos")]
pub struct TodoView {
    #[id("todo_id")]
    pub todo_id: String,
    pub owner_id: String,
    pub title: String,
    /// `open` | `completed` | `archived`
    pub status: String,
}

/// Chat message row. PK: `message_id`. Live via GraphQL subscription on `chat_messages`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ReadModel)]
#[table("chat_messages")]
pub struct ChatMessageView {
    #[id("message_id")]
    pub message_id: String,
    pub room_id: String,
    pub author_id: String,
    pub body: String,
    pub created_at: String,
}

pub fn map_todo_fact(e: &TodoFact) -> TodoView {
    TodoView {
        todo_id: e.todo_id.clone(),
        owner_id: e.owner_id.clone(),
        title: e.title.clone(),
        status: e.status.clone(),
    }
}

pub fn map_chat_posted(e: &ChatMessagePosted) -> ChatMessageView {
    ChatMessageView {
        message_id: e.message_id.clone(),
        room_id: e.room_id.clone(),
        author_id: e.author_id.clone(),
        body: e.body.clone(),
        created_at: e.created_at.clone(),
    }
}

// Back-compat alias used by todo projector.
pub use map_todo_fact as map_fact;

pub fn distributed_manifest() -> distributed::DistributedProjectManifest {
    use distributed::RelationalReadModel;
    distributed::DistributedProjectManifest::new("e2e-ui")
        .table_schema(TodoView::schema().clone())
        .table_schema(ChatMessageView::schema().clone())
}
