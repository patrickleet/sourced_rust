use serde::{de::DeserializeOwned, Serialize};

use crate::aggregate::Aggregate;

/// Opt-in trait for aggregates that can produce state snapshot payloads.
///
/// Aggregates implementing this trait can have their state captured as a DTO at
/// a point in time and restored later. Repositories may serialize that payload
/// into a snapshot cache record to skip costly full event replay, but the
/// payload itself is not an aggregate event and is not durable history by
/// itself.
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
