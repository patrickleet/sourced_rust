use serde::{de::DeserializeOwned, Serialize};

use crate::aggregate::Aggregate;

/// Opt-in trait for aggregates that support snapshot-based hydration.
///
/// Aggregates implementing this trait can have their state serialized at a point
/// in time and restored later, skipping costly full event replay.
///
/// The associated `Snapshot` type is a separate struct (e.g., `TodoSnapshot`)
/// that captures the aggregate's current state.
pub trait Snapshottable: Aggregate {
    type Snapshot: Serialize + DeserializeOwned;

    /// Create a snapshot of the current aggregate state.
    fn create_snapshot(&self) -> Self::Snapshot;

    /// Restore aggregate state from a snapshot.
    fn restore_from_snapshot(&mut self, snapshot: Self::Snapshot);
}
