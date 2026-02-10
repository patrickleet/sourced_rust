mod in_memory;
mod repository;
mod snapshottable;
mod store;

pub use in_memory::InMemorySnapshotStore;
pub use repository::{hydrate_from_snapshot, SnapshotAggregateRepository};
pub use snapshottable::Snapshottable;
pub use store::{SnapshotRecord, SnapshotStore};
