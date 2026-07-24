//! Trait aliases for generic handler storage parameters.

use distributed::microsvc::{CausalProjectionStore, CausalRepositoryBackend};
use distributed::{
    GetStream, LockManager, ReadModelWritePlanStore, RelationalReadModelQueryStore,
    TransactionalCommit,
};

pub trait EventStore:
    CausalRepositoryBackend + GetStream + TransactionalCommit + Clone + Send + Sync + 'static
{
}
impl<T> EventStore for T where
    T: CausalRepositoryBackend + GetStream + TransactionalCommit + Clone + Send + Sync + 'static
{
}

pub trait Locks: LockManager + Clone + 'static {}
impl<T> Locks for T where T: LockManager + Clone + 'static {}

pub trait ReadStore:
    CausalProjectionStore
    + ReadModelWritePlanStore
    + RelationalReadModelQueryStore
    + Clone
    + Send
    + Sync
    + 'static
{
}
impl<T> ReadStore for T where
    T: CausalProjectionStore
        + ReadModelWritePlanStore
        + RelationalReadModelQueryStore
        + Clone
        + Send
        + Sync
        + 'static
{
}
