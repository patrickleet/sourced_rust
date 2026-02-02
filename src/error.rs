use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepositoryError {
    LockPoisoned(&'static str),
    ConcurrentWrite {
        id: String,
        expected: u64,
        actual: u64,
    },
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
        }
    }
}

impl std::error::Error for RepositoryError {}
