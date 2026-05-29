use std::future::Future;

use super::LockError;

/// Async counterpart to [`Lock`](super::Lock): a single lock instance whose
/// acquisition yields to the executor instead of blocking the OS thread.
///
/// Only `lock` is asynchronous — it must `.await` (without blocking the
/// executor) until the lock becomes free. `try_lock` and `unlock` only inspect
/// or mutate lock state and wake waiters, so they stay synchronous and
/// non-blocking, mirroring the sync [`Lock`](super::Lock) trait.
///
/// The returned future is `Send` so an async `QueuedRepository` built on this
/// lock keeps its repository futures `Send` (required by the async repo traits).
pub trait AsyncLock: Send + Sync {
    /// Acquire the lock, awaiting until it becomes available.
    fn lock(&self) -> impl Future<Output = Result<(), LockError>> + Send + '_;

    /// Try to acquire the lock without waiting.
    ///
    /// Returns `Ok(true)` if acquired, `Ok(false)` if already held.
    fn try_lock(&self) -> Result<bool, LockError>;

    /// Release the lock, waking any waiters so they can re-contend.
    fn unlock(&self) -> Result<(), LockError>;
}
