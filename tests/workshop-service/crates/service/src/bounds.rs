//! Trait aliases for generic handler storage parameters.

use distributed::{
    GetStream, LockManager, ReadModelWritePlanStore, RelationalReadModelQueryStore,
    TransactionalCommit,
};

/// Event-store leaf usable under `QueuedRepository` + `AggregateRepository`.
pub trait EventStore: GetStream + TransactionalCommit + Clone + Send + Sync + 'static {}

impl<T> EventStore for T where T: GetStream + TransactionalCommit + Clone + Send + Sync + 'static {}

/// Lock manager for `queued_with`.
pub trait Locks: LockManager + Clone + 'static {}

impl<T> Locks for T where T: LockManager + Clone + 'static {}

/// Read-model store for projectors.
pub trait ReadStore:
    ReadModelWritePlanStore + RelationalReadModelQueryStore + Clone + Send + Sync + 'static
{
}

impl<T> ReadStore for T where
    T: ReadModelWritePlanStore + RelationalReadModelQueryStore + Clone + Send + Sync + 'static
{
}
