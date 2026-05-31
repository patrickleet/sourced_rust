//! Transport error classification.
//!
//! Async transport adapters and the runner that drives them need a shared way
//! to say whether a failure is worth retrying. [`TransportError`] carries that
//! classification so the runner can decide between negative-acknowledging a
//! message for redelivery (retryable) and handing it to the configured
//! [`FailurePolicy`](super::FailurePolicy) (permanent).

use std::error::Error;
use std::fmt;

use crate::microsvc::HandlerError;

/// Whether a [`TransportError`] should be retried.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum TransportErrorKind {
    /// A transient failure. The message should be redelivered and retried.
    ///
    /// Unknown outcomes (for example, a publish whose acknowledgement was lost)
    /// are retryable: duplicate delivery is acceptable under at-least-once
    /// semantics, but silently dropping the work is not.
    Retryable,
    /// A deterministic failure. Retrying the same message as-is will not help,
    /// so the runner consults the failure policy (dead-letter, park,
    /// log-and-ack, or stop) instead of redelivering forever.
    Permanent,
}

impl TransportErrorKind {
    /// Whether this kind is retryable.
    pub fn is_retryable(self) -> bool {
        matches!(self, TransportErrorKind::Retryable)
    }

    /// Whether this kind is permanent.
    pub fn is_permanent(self) -> bool {
        matches!(self, TransportErrorKind::Permanent)
    }
}

/// An error raised by a transport adapter or runner, classified as retryable or
/// permanent.
///
/// The classification is the contract the runner relies on; the message and
/// optional source are for diagnostics, logging, and dead-letter metadata.
#[derive(Debug)]
pub struct TransportError {
    kind: TransportErrorKind,
    message: String,
    source: Option<Box<dyn Error + Send + Sync>>,
}

impl TransportError {
    /// Create a retryable transport error.
    pub fn retryable(message: impl Into<String>) -> Self {
        Self {
            kind: TransportErrorKind::Retryable,
            message: message.into(),
            source: None,
        }
    }

    /// Create a permanent transport error.
    pub fn permanent(message: impl Into<String>) -> Self {
        Self {
            kind: TransportErrorKind::Permanent,
            message: message.into(),
            source: None,
        }
    }

    /// Create a transport error with an explicit classification.
    pub fn new(kind: TransportErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            source: None,
        }
    }

    /// Attach an underlying source error for diagnostics.
    pub fn with_source(mut self, source: impl Error + Send + Sync + 'static) -> Self {
        self.source = Some(Box::new(source));
        self
    }

    /// The retry classification of this error.
    pub fn kind(&self) -> TransportErrorKind {
        self.kind
    }

    /// Whether this error is retryable.
    pub fn is_retryable(&self) -> bool {
        self.kind.is_retryable()
    }

    /// Whether this error is permanent.
    pub fn is_permanent(&self) -> bool {
        self.kind.is_permanent()
    }

    /// The human-readable message, without the classification prefix.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Classify a [`HandlerError`] for transport retry purposes.
    ///
    /// Transient failures are retryable: repository errors, not-found results,
    /// and otherwise-unclassified errors. Not-found is transient in an
    /// at-least-once event-driven system because an out-of-order delivery can
    /// reference an aggregate or projection that has not been created yet, and a
    /// later redelivery resolves the race. (This also keeps a handler-raised
    /// [`HandlerError::NotFound`] consistent with a repository-raised
    /// [`RepositoryError::NotFound`](crate::RepositoryError), which arrives
    /// wrapped in `Repository(_)`.)
    ///
    /// Deterministic failures — unknown routing, payload decode failures,
    /// business rejections, authorization, and guard rejections — are permanent
    /// because redelivering the identical message cannot change the outcome. An
    /// entity that is genuinely missing forever still drains to the failure
    /// policy once the transport's retry ceiling is reached, so defaulting
    /// not-found to retryable does not risk an unbounded loop.
    ///
    /// This is a default. Handlers that know better can return a classification
    /// directly, and the runner's [`FailurePolicy`](super::FailurePolicy) can
    /// override what happens to a permanent error.
    pub fn classify_handler_error(error: &HandlerError) -> TransportErrorKind {
        match error {
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

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self.kind {
            TransportErrorKind::Retryable => "retryable",
            TransportErrorKind::Permanent => "permanent",
        };
        write!(f, "transport error ({kind}): {}", self.message)
    }
}

impl Error for TransportError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source
            .as_ref()
            .map(|source| source.as_ref() as &(dyn Error + 'static))
    }
}

impl From<HandlerError> for TransportError {
    fn from(error: HandlerError) -> Self {
        let kind = Self::classify_handler_error(&error);
        Self {
            kind,
            message: error.to_string(),
            source: Some(Box::new(error)),
        }
    }
}

impl From<crate::RepositoryError> for TransportError {
    /// Repository/store failures (lock contention, storage hiccups, stale-claim
    /// conflicts) are treated as retryable: they are usually transient, and a
    /// later redelivery or re-claim resolves them.
    fn from(error: crate::RepositoryError) -> Self {
        Self {
            kind: TransportErrorKind::Retryable,
            message: error.to_string(),
            source: Some(Box::new(error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retryable_and_permanent_classify_themselves() {
        let retry = TransportError::retryable("connection reset");
        assert!(retry.is_retryable());
        assert!(!retry.is_permanent());
        assert_eq!(retry.kind(), TransportErrorKind::Retryable);

        let permanent = TransportError::permanent("bad payload");
        assert!(permanent.is_permanent());
        assert!(!permanent.is_retryable());
        assert_eq!(permanent.kind(), TransportErrorKind::Permanent);
    }

    #[test]
    fn display_includes_classification_and_message() {
        assert_eq!(
            TransportError::retryable("lease lost").to_string(),
            "transport error (retryable): lease lost"
        );
        assert_eq!(
            TransportError::permanent("decode failed").to_string(),
            "transport error (permanent): decode failed"
        );
    }

    #[test]
    fn with_source_is_exposed_through_error_trait() {
        let inner = std::io::Error::new(std::io::ErrorKind::TimedOut, "timed out");
        let err = TransportError::retryable("publish timed out").with_source(inner);
        assert!(err.source().is_some());
        assert_eq!(err.message(), "publish timed out");
    }

    #[test]
    fn transient_handler_errors_are_retryable() {
        use crate::repository::RepositoryError;

        for error in [
            HandlerError::Repository(RepositoryError::NotFound { id: "agg-1".into() }),
            // A handler-raised not-found is transient too: it is usually an
            // out-of-order delivery race, and stays consistent with the
            // repository-raised not-found above.
            HandlerError::NotFound("agg-1".into()),
            HandlerError::Other("boom".into()),
        ] {
            assert_eq!(
                TransportError::classify_handler_error(&error),
                TransportErrorKind::Retryable,
                "{error} should be retryable"
            );
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
            assert_eq!(
                TransportError::classify_handler_error(&error),
                TransportErrorKind::Permanent,
                "{error} should be permanent"
            );
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
