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

    /// Schema version of the `Snapshot` payload layout.
    ///
    /// Snapshot payloads are encoded with bitcode, which is positional and
    /// **not** self-describing: a layout-compatible change (e.g. reordering two
    /// same-typed fields, or repurposing a field) decodes *successfully* into
    /// the wrong state. A decode error already falls back to replay safely, but
    /// a silent mis-decode would corrupt the aggregate and then commit new
    /// events atop that corruption.
    ///
    /// Bump this constant whenever the `Snapshot` layout changes in a way that
    /// is not guaranteed to decode identically. On load, a stored snapshot whose
    /// version differs from this constant is treated as a cache miss and the
    /// aggregate is rebuilt by full replay (correct, just slower) rather than
    /// decoded into possibly-wrong state.
    ///
    /// Defaults to 1 so existing implementations are unaffected.
    const SNAPSHOT_VERSION: u64 = 1;

    /// Create a snapshot of the current aggregate state.
    fn create_snapshot(&self) -> Self::Snapshot;

    /// Restore aggregate state from a snapshot.
    fn restore_from_snapshot(&mut self, snapshot: Self::Snapshot);
}
