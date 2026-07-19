use serde::{Deserialize, Serialize};

use super::{Todo, TodoStatus};

/// Portable outbox / projection DTO.
/// Full snapshot fields so projectors can upsert without loading prior rows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoFact {
    pub todo_id: String,
    pub owner_id: String,
    pub title: String,
    /// `open` | `completed` | `archived`
    pub status: String,
}

impl TodoFact {
    pub fn from_todo(t: &Todo) -> Self {
        Self {
            todo_id: t.todo_id.clone(),
            owner_id: t.owner_id.clone(),
            title: t.title.clone(),
            status: match t.status {
                TodoStatus::Open => "open".into(),
                TodoStatus::Completed => "completed".into(),
                TodoStatus::Archived => "archived".into(),
            },
        }
    }
}
