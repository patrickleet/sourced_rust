//! Error types for microsvc command handlers.

use std::error::Error;
use std::fmt;

use crate::bus::{PayloadDecodeError, TransportError, TransportErrorKind};
use crate::{repository::RepositoryError, EventRecordError};

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

impl From<EventRecordError> for HandlerError {
    fn from(err: EventRecordError) -> Self {
        HandlerError::Other(Box::new(err))
    }
}

impl From<serde_json::Error> for HandlerError {
    fn from(err: serde_json::Error) -> Self {
        HandlerError::DecodeFailed(err.to_string())
    }
}

impl From<PayloadDecodeError> for HandlerError {
    fn from(err: PayloadDecodeError) -> Self {
        HandlerError::DecodeFailed(err.0)
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

    /// Message safe to return to an untrusted client across a transport.
    ///
    /// Server-internal failures (status 5xx — repository/driver/other errors)
    /// are masked to a generic string so SQL text, driver detail, or internal
    /// paths never leak to callers. Client-fault errors (4xx — unknown command,
    /// decode, rejection, auth, guard) keep their descriptive message because
    /// the caller caused them and the detail helps them correct the request.
    ///
    /// The HTTP, gRPC, and Knative ingresses route error bodies through this so
    /// the masking policy lives in exactly one place.
    pub fn client_facing_message(&self) -> String {
        if self.status_code() >= 500 {
            "Internal server error".to_string()
        } else {
            self.to_string()
        }
    }

    /// Classify this error for transport retry purposes (retryable vs permanent).
    ///
    /// Transient failures (repository errors, not-found, otherwise-unclassified)
    /// are retryable: in an at-least-once event-driven system a not-found is
    /// usually an out-of-order delivery race a later redelivery resolves.
    /// Deterministic failures (unknown routing, decode, rejection, auth, guard)
    /// are permanent — redelivering the identical message cannot change them.
    pub(crate) fn transport_error_kind(&self) -> TransportErrorKind {
        match self {
            HandlerError::Repository(_) | HandlerError::NotFound(_) | HandlerError::Other(_) => {
                TransportErrorKind::Retryable
            }
            HandlerError::UnknownCommand(_)
            | HandlerError::DecodeFailed(_)
            | HandlerError::Rejected(_)
            | HandlerError::Unauthorized(_)
            | HandlerError::GuardRejected(_) => TransportErrorKind::Permanent,
        }
    }
}

/// Classify and convert a handler error into the transport's retryable/permanent
/// vocabulary. This lives on the microsvc side (not the bus) so the bus core does
/// not depend on `HandlerError`.
impl From<HandlerError> for TransportError {
    fn from(error: HandlerError) -> Self {
        let kind = error.transport_error_kind();
        TransportError::new(kind, error.to_string()).with_source(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transient_handler_errors_are_retryable() {
        for error in [
            HandlerError::Repository(RepositoryError::NotFound { id: "agg-1".into() }),
            HandlerError::NotFound("agg-1".into()),
            HandlerError::Other("boom".into()),
        ] {
            assert_eq!(error.transport_error_kind(), TransportErrorKind::Retryable);
        }
    }

    #[test]
    fn deterministic_handler_errors_are_permanent() {
        for error in [
            HandlerError::UnknownCommand("x".into()),
            HandlerError::DecodeFailed("x".into()),
            HandlerError::Rejected("x".into()),
            HandlerError::Unauthorized("x".into()),
            HandlerError::GuardRejected("x".into()),
        ] {
            assert_eq!(error.transport_error_kind(), TransportErrorKind::Permanent);
        }
    }

    #[test]
    fn from_handler_error_preserves_classification_and_source() {
        let err: TransportError = HandlerError::Rejected("invalid".into()).into();
        assert!(err.is_permanent());
        assert!(err.source().is_some());

        let err: TransportError =
            HandlerError::Other(Box::<dyn Error + Send + Sync>::from("infra")).into();
        assert!(err.is_retryable());
    }
}
