use std::fmt;

use crate::model::ModelError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepositoryError {
    LockPoisoned(&'static str),
    ConcurrentWrite {
        id: String,
        expected: u64,
        actual: u64,
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
            RepositoryError::ConcurrentWrite {
                id,
                expected,
                actual,
            } => write!(
                f,
                "concurrent write detected for entity {} (expected version {}, got {})",
                id, expected, actual
            ),
            RepositoryError::Replay(message) => write!(f, "replay error: {}", message),
            RepositoryError::Model(message) => write!(f, "model error: {}", message),
        }
    }
}

impl std::error::Error for RepositoryError {}

impl From<ModelError> for RepositoryError {
    fn from(err: ModelError) -> Self {
        RepositoryError::Model(err.to_string())
    }
}
