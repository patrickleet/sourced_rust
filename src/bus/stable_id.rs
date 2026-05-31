//! Stable message id validation for inbox-enabled execution.
//!
//! Idempotent consumers tolerate a missing [`Message.id`](super::super::Message)
//! because applying the same projection twice yields the same state. A consumer
//! *inbox*, however, deduplicates by a durable key, so inbox-enabled runs must
//! reject messages whose id cannot serve as that key.
//!
//! These rules define what counts as a usable stable id. The runner and the
//! future inbox implementation share them so "no stable id" is rejected
//! consistently rather than producing a silently broken dedup key.

use std::error::Error;
use std::fmt;

/// Maximum length, in bytes, of a stable message id.
///
/// Bounds the size of dedup keys an inbox must store and index. CloudEvents
/// `id`, Kafka event-id headers, and outbox `message_id` values all fit well
/// under this ceiling.
pub const MAX_STABLE_MESSAGE_ID_LEN: usize = 512;

/// Why a candidate stable message id is not usable for inbox deduplication.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum StableMessageIdError {
    /// The message had no id at all (`None`).
    Missing,
    /// The id was present but empty or only whitespace.
    Empty,
    /// The id exceeded [`MAX_STABLE_MESSAGE_ID_LEN`].
    TooLong {
        /// The actual length, in bytes.
        len: usize,
    },
    /// The id contained a control character (newline, NUL, etc.), which would
    /// corrupt keys, logs, or headers.
    InvalidCharacter,
}

impl fmt::Display for StableMessageIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StableMessageIdError::Missing => {
                write!(f, "stable message id is required but missing")
            }
            StableMessageIdError::Empty => {
                write!(f, "stable message id is empty")
            }
            StableMessageIdError::TooLong { len } => write!(
                f,
                "stable message id is {len} bytes, exceeding the maximum of {MAX_STABLE_MESSAGE_ID_LEN}"
            ),
            StableMessageIdError::InvalidCharacter => {
                write!(f, "stable message id contains a control character")
            }
        }
    }
}

impl Error for StableMessageIdError {}

/// Validate a candidate stable message id for inbox-enabled execution.
///
/// Returns the borrowed id when it is usable as a deduplication key. A missing,
/// empty, over-long, or control-character-bearing id is rejected. Surrounding
/// whitespace is checked for emptiness but the original value is returned
/// unchanged so callers key on exactly what the transport delivered.
pub fn validate_stable_message_id(id: Option<&str>) -> Result<&str, StableMessageIdError> {
    let id = id.ok_or(StableMessageIdError::Missing)?;
    if id.trim().is_empty() {
        return Err(StableMessageIdError::Empty);
    }
    if id.len() > MAX_STABLE_MESSAGE_ID_LEN {
        return Err(StableMessageIdError::TooLong { len: id.len() });
    }
    if id.chars().any(|c| c.is_control()) {
        return Err(StableMessageIdError::InvalidCharacter);
    }
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_a_well_formed_id() {
        assert_eq!(validate_stable_message_id(Some("evt-1")), Ok("evt-1"));
        // Surrounding whitespace is preserved in the returned value.
        assert_eq!(validate_stable_message_id(Some(" evt-1 ")), Ok(" evt-1 "));
    }

    #[test]
    fn rejects_missing_id() {
        assert_eq!(
            validate_stable_message_id(None),
            Err(StableMessageIdError::Missing)
        );
    }

    #[test]
    fn rejects_empty_or_whitespace_id() {
        assert_eq!(
            validate_stable_message_id(Some("")),
            Err(StableMessageIdError::Empty)
        );
        assert_eq!(
            validate_stable_message_id(Some("   ")),
            Err(StableMessageIdError::Empty)
        );
    }

    #[test]
    fn rejects_over_long_id() {
        let id = "a".repeat(MAX_STABLE_MESSAGE_ID_LEN + 1);
        assert_eq!(
            validate_stable_message_id(Some(&id)),
            Err(StableMessageIdError::TooLong {
                len: MAX_STABLE_MESSAGE_ID_LEN + 1
            })
        );
        // The boundary length is accepted.
        let boundary = "a".repeat(MAX_STABLE_MESSAGE_ID_LEN);
        assert!(validate_stable_message_id(Some(&boundary)).is_ok());
    }

    #[test]
    fn rejects_control_characters() {
        assert_eq!(
            validate_stable_message_id(Some("evt\n1")),
            Err(StableMessageIdError::InvalidCharacter)
        );
        assert_eq!(
            validate_stable_message_id(Some("evt\u{0}1")),
            Err(StableMessageIdError::InvalidCharacter)
        );
        assert_eq!(
            validate_stable_message_id(Some("evt\t1")),
            Err(StableMessageIdError::InvalidCharacter)
        );
    }
}
