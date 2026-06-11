use std::fmt;

/// Whether a failure is worth retrying as-is.
///
/// This mirrors the transport layer's
/// [`TransportErrorKind`](crate::bus::TransportErrorKind): a `Retryable` failure
/// is transient (a later attempt may succeed), a `Permanent` failure is
/// deterministic (re-running the identical operation cannot change the outcome).
/// It lives in the lock module — the lowest layer that needs it — so both
/// [`LockError`] and [`RepositoryError`](crate::repository::RepositoryError) can
/// share one vocabulary without the repository layer depending on the bus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RetryClass {
    /// A transient failure. Retrying the same operation may succeed.
    Retryable,
    /// A deterministic failure. Retrying the identical operation will not help.
    Permanent,
}

impl RetryClass {
    /// Whether this class is retryable.
    pub fn is_retryable(self) -> bool {
        matches!(self, RetryClass::Retryable)
    }

    /// Whether this class is permanent.
    pub fn is_permanent(self) -> bool {
        matches!(self, RetryClass::Permanent)
    }
}

/// Error type for lock operations.
///
/// Each variant carries a retry classification via [`LockError::kind`]:
/// contention and lease loss are transient (retry the acquire), whereas a
/// poisoned primitive is a process-fatal invariant violation that retrying
/// cannot clear.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum LockError {
    /// The underlying lock primitive was poisoned (e.g. a thread panicked while holding it).
    Poisoned(String),
    /// Failed to acquire the lock.
    AcquireFailed(String),
    /// Failed to release the lock.
    ReleaseFailed(String),
    /// The lock expired (e.g. a distributed lock TTL elapsed).
    Expired(String),
    /// A transient backend condition (e.g. SQLite `SQLITE_BUSY`/`SQLITE_LOCKED`
    /// or a pool-acquire timeout) prevented the operation. Retrying may succeed.
    Busy(String),
    /// Any other lock error.
    Other(String),
}

impl LockError {
    /// Classify this error for retry purposes.
    ///
    /// `Poisoned` is permanent — a poisoned primitive signals a broken invariant
    /// that a retry cannot clear. Contention (`AcquireFailed`, `Busy`), lease
    /// loss (`Expired`), and transient release failures are retryable. `Other`
    /// is conservatively retryable: an unclassified lock failure is more often
    /// transient infrastructure than a deterministic bug, and at-least-once
    /// semantics make a redundant retry safe.
    pub fn kind(&self) -> RetryClass {
        match self {
            LockError::Poisoned(_) => RetryClass::Permanent,
            LockError::AcquireFailed(_)
            | LockError::ReleaseFailed(_)
            | LockError::Expired(_)
            | LockError::Busy(_)
            | LockError::Other(_) => RetryClass::Retryable,
        }
    }

    /// Whether this error is retryable.
    pub fn is_retryable(&self) -> bool {
        self.kind().is_retryable()
    }
}

impl fmt::Display for LockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LockError::Poisoned(msg) => write!(f, "lock poisoned: {}", msg),
            LockError::AcquireFailed(msg) => write!(f, "lock acquire failed: {}", msg),
            LockError::ReleaseFailed(msg) => write!(f, "lock release failed: {}", msg),
            LockError::Expired(msg) => write!(f, "lock expired: {}", msg),
            LockError::Busy(msg) => write!(f, "lock busy: {}", msg),
            LockError::Other(msg) => write!(f, "lock error: {}", msg),
        }
    }
}

impl std::error::Error for LockError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contention_and_lease_loss_are_retryable() {
        for err in [
            LockError::AcquireFailed("contended".into()),
            LockError::ReleaseFailed("transient".into()),
            LockError::Expired("ttl elapsed".into()),
            LockError::Busy("database is locked".into()),
            LockError::Other("unknown".into()),
        ] {
            assert_eq!(err.kind(), RetryClass::Retryable, "{err}");
            assert!(err.is_retryable());
        }
    }

    #[test]
    fn poisoned_is_permanent() {
        let err = LockError::Poisoned("map poisoned".into());
        assert_eq!(err.kind(), RetryClass::Permanent);
        assert!(!err.is_retryable());
    }
}
