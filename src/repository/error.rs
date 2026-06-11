use std::error::Error;
use std::fmt;

use crate::lock::{LockError, RetryClass};
use crate::read_model::ReadModelError;
use crate::EventRecordError;

#[derive(Debug)]
#[non_exhaustive]
pub enum RepositoryError {
    LockPoisoned(&'static str),
    Lock(LockError),
    ConcurrentWrite {
        id: String,
        expected: u64,
        actual: u64,
    },
    DuplicateStreamInBatch {
        id: String,
    },
    DuplicateOutboxMessageInBatch {
        id: String,
    },
    /// A consumer inbox receipt `(consumer, message_id)` was already recorded.
    /// The commit is rolled back so the consumer's effects are not double-applied;
    /// the message has already been processed (an at-least-once replay).
    DuplicateInboxReceipt {
        consumer: String,
        message_id: String,
    },
    /// A consumer inbox receipt had an empty `consumer` or `message_id`. Rejected
    /// uniformly across backends before any write (the relational `CHECK`
    /// constraints are a defense-in-depth backstop).
    InvalidInboxReceipt {
        consumer: String,
        message_id: String,
    },
    InvalidStreamIdentity {
        aggregate_type: String,
        aggregate_id: String,
        reason: String,
    },
    NotFound {
        id: String,
    },
    InvalidState {
        id: String,
        expected: &'static str,
        actual: String,
    },
    Replay(String),
    Model(String),
    /// A storage backend (event store, read model, snapshot store) failed to
    /// complete an operation. Unlike [`RepositoryError::Model`] — a deterministic
    /// modeling/decoding fault — this carries an explicit retry classification so
    /// callers can distinguish a transient outage (connection refused, pool
    /// timeout, `SQLITE_BUSY`) from a deterministic failure (constraint
    /// violation, malformed row) without string-sniffing the message.
    ///
    /// The optional `source` preserves the underlying error for diagnostics and
    /// dead-letter metadata; it is exposed through [`Error::source`].
    Storage {
        /// The operation that failed (e.g. `"sqlite insert event"`).
        operation: String,
        /// Whether retrying the same operation may succeed.
        retryable: bool,
        /// The underlying backend error, if available.
        source: Option<Box<dyn Error + Send + Sync>>,
    },
}

impl RepositoryError {
    /// Construct a retryable storage failure carrying its source.
    pub fn retryable_storage(
        operation: impl Into<String>,
        source: impl Error + Send + Sync + 'static,
    ) -> Self {
        RepositoryError::Storage {
            operation: operation.into(),
            retryable: true,
            source: Some(Box::new(source)),
        }
    }

    /// Construct a permanent storage failure carrying its source.
    pub fn permanent_storage(
        operation: impl Into<String>,
        source: impl Error + Send + Sync + 'static,
    ) -> Self {
        RepositoryError::Storage {
            operation: operation.into(),
            retryable: false,
            source: Some(Box::new(source)),
        }
    }

    /// Classify this error for retry purposes.
    ///
    /// The contract a runner relies on: a retryable error should be redelivered
    /// (a later attempt may succeed); a permanent error should not, because
    /// re-running the identical operation cannot change a deterministic outcome.
    ///
    /// - `Storage { retryable, .. }` reports the classification captured when the
    ///   backend error was mapped (connection/pool/timeout → retryable;
    ///   constraint/decode → permanent).
    /// - `Lock` defers to [`LockError::kind`].
    /// - `ConcurrentWrite` is **retryable**: an optimistic-concurrency conflict
    ///   means another writer won the race; reloading and reapplying typically
    ///   succeeds. This preserves the prior behavior where it fell into the
    ///   retryable bucket.
    /// - `NotFound` is retryable: under at-least-once delivery it is usually an
    ///   out-of-order race a later redelivery resolves.
    /// - The deterministic faults (`Model`, `Replay`, invalid identity/receipt,
    ///   `InvalidState`, duplicate-in-batch, `LockPoisoned`) are permanent.
    pub fn kind(&self) -> RetryClass {
        match self {
            RepositoryError::Storage { retryable, .. } => {
                if *retryable {
                    RetryClass::Retryable
                } else {
                    RetryClass::Permanent
                }
            }
            RepositoryError::Lock(err) => err.kind(),
            RepositoryError::ConcurrentWrite { .. } | RepositoryError::NotFound { .. } => {
                RetryClass::Retryable
            }
            RepositoryError::LockPoisoned(_)
            | RepositoryError::DuplicateStreamInBatch { .. }
            | RepositoryError::DuplicateOutboxMessageInBatch { .. }
            | RepositoryError::DuplicateInboxReceipt { .. }
            | RepositoryError::InvalidInboxReceipt { .. }
            | RepositoryError::InvalidStreamIdentity { .. }
            | RepositoryError::InvalidState { .. }
            | RepositoryError::Replay(_)
            | RepositoryError::Model(_) => RetryClass::Permanent,
        }
    }

    /// Whether this error is retryable.
    pub fn is_retryable(&self) -> bool {
        self.kind().is_retryable()
    }
}

impl fmt::Display for RepositoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RepositoryError::LockPoisoned(operation) => {
                write!(f, "repository lock poisoned during {}", operation)
            }
            RepositoryError::Lock(err) => write!(f, "repository lock error: {}", err),
            RepositoryError::ConcurrentWrite {
                id,
                expected,
                actual,
            } => write!(
                f,
                "concurrent write detected for entity {} (expected version {}, got {})",
                id, expected, actual
            ),
            RepositoryError::DuplicateStreamInBatch { id } => {
                write!(f, "duplicate stream id in commit batch: {}", id)
            }
            RepositoryError::DuplicateOutboxMessageInBatch { id } => {
                write!(f, "duplicate outbox message id in commit batch: {}", id)
            }
            RepositoryError::DuplicateInboxReceipt {
                consumer,
                message_id,
            } => write!(
                f,
                "consumer inbox receipt already recorded for consumer `{}`, message `{}`",
                consumer, message_id
            ),
            RepositoryError::InvalidInboxReceipt {
                consumer,
                message_id,
            } => write!(
                f,
                "invalid consumer inbox receipt (consumer `{}`, message `{}`): consumer and message id must be non-empty",
                consumer, message_id
            ),
            RepositoryError::InvalidStreamIdentity {
                aggregate_type,
                aggregate_id,
                reason,
            } => write!(
                f,
                "invalid stream identity (type `{}`, id `{}`): {}",
                aggregate_type, aggregate_id, reason
            ),
            RepositoryError::NotFound { id } => write!(f, "entity not found: {}", id),
            RepositoryError::InvalidState {
                id,
                expected,
                actual,
            } => write!(
                f,
                "invalid state for entity {} (expected {}, got {})",
                id, expected, actual
            ),
            RepositoryError::Replay(message) => write!(f, "replay error: {}", message),
            RepositoryError::Model(message) => write!(f, "model error: {}", message),
            RepositoryError::Storage {
                operation,
                retryable,
                source,
            } => {
                let class = if *retryable { "retryable" } else { "permanent" };
                match source {
                    Some(source) => {
                        write!(f, "storage error ({class}) during {operation}: {source}")
                    }
                    None => write!(f, "storage error ({class}) during {operation}"),
                }
            }
        }
    }
}

impl Error for RepositoryError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            RepositoryError::Lock(err) => Some(err),
            RepositoryError::Storage {
                source: Some(source),
                ..
            } => Some(source.as_ref()),
            _ => None,
        }
    }
}

impl From<LockError> for RepositoryError {
    fn from(err: LockError) -> Self {
        RepositoryError::Lock(err)
    }
}

impl From<ReadModelError> for RepositoryError {
    fn from(err: ReadModelError) -> Self {
        // Map to `Storage` so the read-model error keeps a retry signal and its
        // source instead of collapsing to an opaque `Model` string. Only a lock
        // failure carries a transient/permanent distinction we can recover here;
        // every other read-model variant is deterministic (a concurrency
        // conflict, serde/metadata fault, or not-found will fail the same way on
        // redelivery). `ReadModelError::Storage` is itself a stringified backend
        // error with no preserved retry signal — without changing `read_model`
        // it is classified permanent, which is the safe default (it cannot loop
        // forever; it surfaces to the failure policy).
        let retryable = matches!(&err, ReadModelError::Lock(lock) if lock.is_retryable());
        RepositoryError::Storage {
            operation: "read model".into(),
            retryable,
            source: Some(Box::new(err)),
        }
    }
}

impl From<EventRecordError> for RepositoryError {
    fn from(err: EventRecordError) -> Self {
        // Event (de)serialization faults are deterministic: the same bytes will
        // fail the same way on redelivery. Classify as permanent storage, but
        // preserve the source for diagnostics rather than stringifying it away.
        RepositoryError::Storage {
            operation: "event record".into(),
            retryable: false,
            source: Some(Box::new(err)),
        }
    }
}
