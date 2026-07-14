//! Shared GraphQL command payload types for todo lifecycle mutations.

use serde::Serialize;

/// Common complete / archive / reopen GraphQL payload.
#[derive(Debug, Serialize, distributed::GraphqlOutput)]
pub struct TodoStatusPayload {
    pub todo_id: String,
    pub status: String,
}
