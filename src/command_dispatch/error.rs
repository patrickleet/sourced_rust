use std::fmt;

/// Fail-closed dispatch errors shared by local and remote adapters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandDispatchError {
    /// Envelope version/codec/fingerprint rejected before execution.
    Rejected(String),
    /// Destination unknown or ambiguous for the selected process plan.
    Unroutable(String),
    /// Remote transport or trust failure (no ambiguous success).
    Transport(String),
    /// Deadline exceeded before a durable outcome was known.
    DeadlineExceeded,
    /// Handler/domain failure mapped through the approved contract.
    Handler(String),
}

impl fmt::Display for CommandDispatchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rejected(reason) => write!(formatter, "command rejected: {reason}"),
            Self::Unroutable(reason) => write!(formatter, "command unroutable: {reason}"),
            Self::Transport(reason) => write!(formatter, "command transport error: {reason}"),
            Self::DeadlineExceeded => write!(formatter, "command dispatch deadline exceeded"),
            Self::Handler(reason) => write!(formatter, "command handler error: {reason}"),
        }
    }
}

impl std::error::Error for CommandDispatchError {}
