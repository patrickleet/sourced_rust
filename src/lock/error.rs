use std::fmt;

/// Error type for lock operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockError {
    /// The underlying lock primitive was poisoned (e.g. a thread panicked while holding it).
    Poisoned(String),
    /// Failed to acquire the lock.
    AcquireFailed(String),
    /// Failed to release the lock.
    ReleaseFailed(String),
    /// The lock expired (e.g. a distributed lock TTL elapsed).
    Expired(String),
    /// Any other lock error.
    Other(String),
}

impl fmt::Display for LockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LockError::Poisoned(msg) => write!(f, "lock poisoned: {}", msg),
            LockError::AcquireFailed(msg) => write!(f, "lock acquire failed: {}", msg),
            LockError::ReleaseFailed(msg) => write!(f, "lock release failed: {}", msg),
            LockError::Expired(msg) => write!(f, "lock expired: {}", msg),
            LockError::Other(msg) => write!(f, "lock error: {}", msg),
        }
    }
}

impl std::error::Error for LockError {}
