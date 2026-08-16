//! Command: `todo.complete` — owner-only (aggregate enforces).
//!
//! The executable path is `load_by` + `invoke` + `eventual` on the module
//! route. This file owns the command identity and GraphQL input only.

use serde::Deserialize;

pub const COMMAND: &str = "todo.complete";

#[derive(Debug, Deserialize, distributed::GraphqlInput)]
pub struct TodoCompleteInput {
    pub todo_id: String,
}
