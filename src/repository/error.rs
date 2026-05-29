use std::fmt;

use crate::lock::LockError;
use crate::read_model::ReadModelError;
use crate::EventRecordError;

#[derive(Debug, Clone, PartialEq, Eq)]
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
        }
    }
}

impl std::error::Error for RepositoryError {}

impl From<LockError> for RepositoryError {
    fn from(err: LockError) -> Self {
        RepositoryError::Lock(err)
    }
}

impl From<ReadModelError> for RepositoryError {
    fn from(err: ReadModelError) -> Self {
        RepositoryError::Model(err.to_string())
    }
}

impl From<EventRecordError> for RepositoryError {
    fn from(err: EventRecordError) -> Self {
        RepositoryError::Model(err.to_string())
    }
}
