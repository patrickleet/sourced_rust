//! Error types for microsvc command handlers.

use std::error::Error;
use std::fmt;

use crate::repository::RepositoryError;

/// Error type for command handler operations.
#[derive(Debug)]
pub enum HandlerError {
    /// No handler registered for this command name.
    UnknownCommand(String),
    /// Payload decode / deserialization failed.
    DecodeFailed(String),
    /// Business logic rejected the command (validation, invariant violation).
    Rejected(String),
    /// Aggregate or resource not found.
    NotFound(String),
    /// Missing or invalid authentication / authorization.
    Unauthorized(String),
    /// Repository error (event store, snapshots, read models).
    Repository(RepositoryError),
    /// Guard rejected the command (input validation failed).
    GuardRejected(String),
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
            HandlerError::Unauthorized(msg) => write!(f, "unauthorized: {}", msg),
            HandlerError::Repository(e) => write!(f, "repository error: {}", e),
            HandlerError::GuardRejected(name) => {
                write!(f, "guard rejected command: {}", name)
            }
            HandlerError::Other(e) => write!(f, "handler error: {}", e),
        }
    }
}

impl Error for HandlerError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            HandlerError::Repository(e) => Some(e),
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

impl From<serde_json::Error> for HandlerError {
    fn from(err: serde_json::Error) -> Self {
        HandlerError::DecodeFailed(err.to_string())
    }
}

impl HandlerError {
    /// Map this error to an HTTP-style status code.
    pub fn status_code(&self) -> u16 {
        match self {
            HandlerError::UnknownCommand(_) => 404,
            HandlerError::DecodeFailed(_) => 400,
            HandlerError::Rejected(_) => 422,
            HandlerError::NotFound(_) => 404,
            HandlerError::Unauthorized(_) => 401,
            HandlerError::Repository(_) => 500,
            HandlerError::GuardRejected(_) => 400,
            HandlerError::Other(_) => 500,
        }
    }
}
