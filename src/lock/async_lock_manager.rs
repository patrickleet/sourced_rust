use std::sync::Arc;

use super::{AsyncLock, LockError};

/// Async counterpart to [`LockManager`](super::LockManager): a factory for
/// per-entity (or per-key) [`AsyncLock`]s.
///
/// An async `QueuedRepository` uses an `AsyncLockManager` to obtain a lock for
/// each aggregate stream. The default [`InMemoryAsyncLockManager`](super::InMemoryAsyncLockManager)
/// stores locks in a `HashMap`; distributed implementations might talk to
/// Redis, Postgres advisory locks, etc.
pub trait AsyncLockManager: Send + Sync {
    /// The concrete async lock type returned by this manager.
    type Lock: AsyncLock;

    /// Get (or create) a lock for the given identifier.
    ///
    /// Repeated calls with the same `id` must return the same logical lock
    /// (i.e. the same `Arc` for in-memory, or the same distributed key).
    fn get_lock(&self, id: &str) -> Result<Arc<Self::Lock>, LockError>;
}
