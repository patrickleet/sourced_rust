use std::collections::HashSet;

use crate::projection_protocol::ResolvedProjectionObligation;

use super::CommandLedgerError;

/// Durable command lifecycle. `Unknown` is intentionally not stored: absence
/// is represented by [`CommandLookup::Unknown`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CommandLedgerState {
    InProgress,
    RetryableUnknown,
    Succeeded,
    SucceededPendingProjection,
    /// Terminal for an **atomic** command (same-tx read-model row sealed).
    Atomic,
    Rejected,
    ProjectionFailed,
    Expired,
}

impl CommandLedgerState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::InProgress => "in_progress",
            Self::RetryableUnknown => "retryable_unknown",
            Self::Succeeded => "succeeded",
            Self::SucceededPendingProjection => "succeeded_pending_projection",
            Self::Atomic => "atomic",
            Self::Rejected => "rejected",
            Self::ProjectionFailed => "projection_failed",
            Self::Expired => "expired",
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self, CommandLedgerError> {
        match value {
            "in_progress" => Ok(Self::InProgress),
            "retryable_unknown" => Ok(Self::RetryableUnknown),
            "succeeded" => Ok(Self::Succeeded),
            "succeeded_pending_projection" => Ok(Self::SucceededPendingProjection),
            "atomic" => Ok(Self::Atomic),
            "rejected" => Ok(Self::Rejected),
            "projection_failed" => Ok(Self::ProjectionFailed),
            "expired" => Ok(Self::Expired),
            other => Err(CommandLedgerError::Corrupt(format!(
                "stored command ledger state `{other}` is invalid"
            ))),
        }
    }

    pub(super) fn is_replayable(self) -> bool {
        matches!(
            self,
            Self::Succeeded
                | Self::SucceededPendingProjection
                | Self::Atomic
                | Self::Rejected
                | Self::ProjectionFailed
        )
    }
}

/// States the command dispatcher may commit through an attempt fence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TerminalCommandState {
    Succeeded,
    SucceededPendingProjection,
    Atomic,
    Rejected,
}

impl From<TerminalCommandState> for CommandLedgerState {
    fn from(value: TerminalCommandState) -> Self {
        match value {
            TerminalCommandState::Succeeded => Self::Succeeded,
            TerminalCommandState::SucceededPendingProjection => Self::SucceededPendingProjection,
            TerminalCommandState::Atomic => Self::Atomic,
            TerminalCommandState::Rejected => Self::Rejected,
        }
    }
}

pub(super) fn validate_projection_obligation_semantics(
    state: CommandLedgerState,
    obligations: &[ResolvedProjectionObligation],
) -> Result<(), String> {
    match state {
        CommandLedgerState::Succeeded
        | CommandLedgerState::Atomic
        | CommandLedgerState::Rejected => {
            if !obligations.is_empty() {
                return Err(format!(
                    "state `{}` must not contain projection obligations",
                    state.as_str()
                ));
            }
        }
        CommandLedgerState::SucceededPendingProjection | CommandLedgerState::ProjectionFailed => {
            if obligations.is_empty() {
                return Err(format!(
                    "state `{}` must contain at least one projection obligation",
                    state.as_str()
                ));
            }
        }
        _ => {
            return Err(format!(
                "state `{}` cannot contain a replay payload",
                state.as_str()
            ));
        }
    }

    for (obligation_index, obligation) in obligations.iter().enumerate() {
        if obligation.projector.trim().is_empty() {
            return Err(format!(
                "projection obligation {obligation_index} has a blank projector"
            ));
        }
        if obligation.model.trim().is_empty() {
            return Err(format!(
                "projection obligation {obligation_index} has a blank model"
            ));
        }
        if obligation.scope.topology().name() != obligation.projector
            || obligation.scope.model() != obligation.model
        {
            return Err(format!(
                "projection obligation {obligation_index} logical projector/model does not match its exact canonical scope"
            ));
        }
        if obligation.key.fields.is_empty() {
            return Err(format!(
                "projection obligation {obligation_index} has no key fields"
            ));
        }

        let mut seen_fields = HashSet::with_capacity(obligation.key.fields.len());
        for (field_index, field) in obligation.key.fields.iter().enumerate() {
            if field.field.trim().is_empty() {
                return Err(format!(
                    "projection obligation {obligation_index} key field {field_index} has a blank name"
                ));
            }
            if !seen_fields.insert(field.field.as_str()) {
                return Err(format!(
                    "projection obligation {obligation_index} contains duplicate key field `{}`",
                    field.field
                ));
            }
        }
    }

    Ok(())
}
