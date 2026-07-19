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

pub fn map_todo_fact(e: &TodoFact) -> TodoView {
    TodoView {
        todo_id: e.todo_id.clone(),
        owner_id: e.owner_id.clone(),
        title: e.title.clone(),
        status: e.status.clone(),
    }
}
