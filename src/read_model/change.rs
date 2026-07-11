//! Read-model change notification seam (always compiled).
//!
//! The emitting side lives in `sqlx_repo` (broadcast + Postgres NOTIFY) and must
//! not depend on the `graphql` feature. Subscriptions consume
//! [`ReadModelChange`] via `SqlxRepository::read_model_changes()` or
//! `GraphqlEngineBuilder::change_stream`.

use std::collections::BTreeSet;

/// Tables touched by a successful read-model write-plan commit.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReadModelChange {
    pub tables: BTreeSet<String>,
}

impl ReadModelChange {
    pub fn new(tables: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            tables: tables.into_iter().map(Into::into).collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.tables.is_empty()
    }
}
