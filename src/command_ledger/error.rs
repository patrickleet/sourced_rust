use std::fmt;

use crate::repository::RepositoryError;

/// Internal ledger error. Conflict is a reservation outcome, not an error, so
/// the dispatcher can map it directly to `COMMAND_ID_REUSE` without inspecting
/// strings or storage errors.
#[derive(Debug)]
pub(crate) enum CommandLedgerError {
    Invalid(String),
    Corrupt(String),
    AttemptFenced { command_id: String },
    Storage(RepositoryError),
}

impl fmt::Display for CommandLedgerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) => write!(formatter, "invalid command ledger value: {message}"),
            Self::Corrupt(message) => write!(formatter, "corrupt command ledger row: {message}"),
            Self::AttemptFenced { command_id } => {
                write!(formatter, "command attempt for `{command_id}` was fenced")
            }
            Self::Storage(error) => write!(formatter, "command ledger storage failed: {error}"),
        }
    }
}

impl std::error::Error for CommandLedgerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Storage(error) => Some(error),
            Self::Invalid(_) | Self::Corrupt(_) | Self::AttemptFenced { .. } => None,
        }
    }
}

impl From<RepositoryError> for CommandLedgerError {
    fn from(error: RepositoryError) -> Self {
        Self::Storage(error)
    }
}
