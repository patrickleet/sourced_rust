//! Error types for domain service command handlers.

use std::error::Error;
use std::fmt;

use crate::bus::PublishError;
use crate::repository::RepositoryError;

/// Error type for command handler operations.
#[derive(Debug)]
pub enum HandlerError {
    /// No handler registered for this command name.
    UnknownCommand(String),
    /// Payload decode failed.
    DecodeFailed(String),
    /// Business logic rejected the command (validation, invariant violation).
    Rejected(String),
    /// Aggregate not found.
    NotFound(String),
    /// Repository error.
    Repository(RepositoryError),
    /// Bus publish error.
    Publish(PublishError),
    /// Other error.
    Other(Box<dyn Error + Send + Sync>),
}

impl fmt::Display for HandlerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HandlerError::UnknownCommand(name) => write!(f, "unknown command: {}", name),
            HandlerError::DecodeFailed(msg) => write!(f, "decode failed: {}", msg),
            HandlerError::Rejected(msg) => write!(f, "rejected: {}", msg),
            HandlerError::NotFound(id) => write!(f, "not found: {}", id),
            HandlerError::Repository(e) => write!(f, "repository error: {}", e),
            HandlerError::Publish(e) => write!(f, "publish error: {}", e),
            HandlerError::Other(e) => write!(f, "handler error: {}", e),
        }
    }
}

impl Error for HandlerError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            HandlerError::Repository(e) => Some(e),
            HandlerError::Publish(e) => Some(e),
            HandlerError::Other(e) => Some(e.as_ref()),
            _ => None,
        }
    }
}

impl From<RepositoryError> for HandlerError {
    fn from(err: RepositoryError) -> Self {
        HandlerError::Repository(err)
    }
}

impl From<PublishError> for HandlerError {
    fn from(err: PublishError) -> Self {
        HandlerError::Publish(err)
    }
}

impl From<bitcode::Error> for HandlerError {
    fn from(err: bitcode::Error) -> Self {
        HandlerError::DecodeFailed(err.to_string())
    }
}

impl From<serde_json::Error> for HandlerError {
    fn from(err: serde_json::Error) -> Self {
        HandlerError::DecodeFailed(err.to_string())
    }
}
