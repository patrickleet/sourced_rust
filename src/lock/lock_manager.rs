use std::sync::Arc;

use super::{Lock, LockError};

/// Factory trait for obtaining per-entity (or per-key) locks.
///
/// `QueuedRepository` and `QueuedReadModelStore` use a `LockManager` to
/// obtain a lock for each entity or read model instance. The default
/// `InMemoryLockManager` stores locks in a `HashMap`; distributed
/// implementations might talk to Redis, Postgres, etc.
pub trait LockManager: Send + Sync {
    /// The concrete lock type returned by this manager.
    type Lock: Lock;

    /// Get (or create) a lock for the given identifier.
    ///
    /// Repeated calls with the same `id` must return the same logical lock
    /// (i.e. the same `Arc` for in-memory, or the same distributed key).
    fn get_lock(&self, id: &str) -> Result<Arc<Self::Lock>, LockError>;
}
