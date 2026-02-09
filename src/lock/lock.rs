use super::LockError;

/// Trait for a single lock instance.
///
/// Implementations provide blocking lock, non-blocking try-lock, and unlock.
/// In-memory locks use `Mutex` + `Condvar`; distributed locks might use
/// Redis, Postgres advisory locks, etcd leases, etc.
pub trait Lock: Send + Sync {
    /// Acquire the lock, blocking until it becomes available.
    fn lock(&self) -> Result<(), LockError>;

    /// Try to acquire the lock without blocking.
    /// Returns `Ok(true)` if acquired, `Ok(false)` if already held.
    fn try_lock(&self) -> Result<bool, LockError>;

    /// Release the lock.
    fn unlock(&self) -> Result<(), LockError>;
}
