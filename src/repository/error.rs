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
