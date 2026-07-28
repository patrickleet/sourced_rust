use distributed::ReadModel;
use serde::{Deserialize, Serialize};

/// Query-oriented Todo row. The natural plural name infers table `todos`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ReadModel)]
#[readmodel(primary_key = ["todo_id"])]
pub struct Todos {
    #[readmodel(id)]
    pub todo_id: String,
    pub owner_id: String,
    pub title: String,
    /// `open` | `completed` | `archived`
    pub status: String,
    pub assignee_id: Option<String>,
}
