use std::fmt;
use std::time::{Duration, SystemTime};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use serde_json::Value;

use crate::projection_protocol::{
    ResolvedProjectionObligation, SameTransactionProjectionBatch, SameTransactionProjectionEvidence,
};
use crate::repository::CommitBatch;

use super::{
    ids::COMMAND_REPLAY_VERSION, state::validate_projection_obligation_semantics, AttemptToken,
    CanonicalInputHash, CausationId, CommandContractFingerprint, CommandId, CommandLedgerError,
    CommandLedgerKey, CommandLedgerState, TerminalCommandState, SHA256_BYTES,
};

/// One validated reservation request. Fresh candidate IDs lose a race safely:
/// only the inserted row keeps them; every retry reads the winner's causation.
pub(crate) struct CommandReservation {
    pub(super) key: CommandLedgerKey,
    pub(super) command_name: String,
    pub(super) contract_fingerprint: CommandContractFingerprint,
    pub(super) input_hash: CanonicalInputHash,
    pub(super) lease: Duration,
    pub(super) retention: Duration,
    pub(super) candidate_causation: CausationId,
    pub(super) candidate_attempt: AttemptToken,
}

impl CommandReservation {
    pub(crate) fn new(
        key: CommandLedgerKey,
        command_name: impl Into<String>,
        contract_fingerprint: CommandContractFingerprint,
        input_hash: CanonicalInputHash,
        lease: Duration,
        retention: Duration,
    ) -> Result<Self, CommandLedgerError> {
        let command_name = command_name.into();
        if command_name.trim().is_empty() {
            return Err(CommandLedgerError::Invalid(
                "command name must not be empty".into(),
            ));
        }
        validate_positive_duration("command attempt lease", lease)?;
        validate_positive_duration("command replay retention", retention)?;
        if retention <= lease {
            return Err(CommandLedgerError::Invalid(
                "command replay retention must be longer than the attempt lease".into(),
            ));
        }
        Ok(Self {
            key,
            command_name,
            contract_fingerprint,
            input_hash,
            lease,
            retention,
            candidate_causation: CausationId::new(),
            candidate_attempt: AttemptToken::new(),
        })
    }

    pub(crate) fn key(&self) -> &CommandLedgerKey {
        &self.key
    }

    pub(crate) fn command_name(&self) -> &str {
        &self.command_name
    }

    pub(crate) fn contract_fingerprint_bytes(&self) -> &[u8; SHA256_BYTES] {
        self.contract_fingerprint.as_bytes()
    }

    pub(crate) fn input_hash_bytes(&self) -> &[u8; SHA256_BYTES] {
        self.input_hash.as_bytes()
    }

    pub(crate) fn lease(&self) -> Duration {
        self.lease
    }

    pub(crate) fn retention(&self) -> Duration {
        self.retention
    }

    pub(crate) fn candidate_causation(&self) -> &CausationId {
        &self.candidate_causation
    }

    pub(crate) fn candidate_attempt(&self) -> &AttemptToken {
        &self.candidate_attempt
    }

    pub(crate) fn acquired_candidate_attempt(&self) -> CommandAttempt {
        CommandAttempt {
            key: self.key.clone(),
            contract_fingerprint: self.contract_fingerprint,
            input_hash: self.input_hash,
            causation_id: self.candidate_causation.clone(),
            attempt_token: self.candidate_attempt.clone(),
            attempt_number: 1,
        }
    }
}

fn validate_positive_duration(label: &str, duration: Duration) -> Result<(), CommandLedgerError> {
    if duration.is_zero() || !duration.as_secs_f64().is_finite() {
        return Err(CommandLedgerError::Invalid(format!(
            "{label} must be a finite positive duration"
        )));
    }
    Ok(())
}

/// Exclusive capability returned to the process that owns the current lease.
/// It is intentionally not `Clone`: one owned value must be consumed by either
/// terminal completion or the retryable-unknown transition.
pub(crate) struct CommandAttempt {
    pub(super) key: CommandLedgerKey,
    pub(super) contract_fingerprint: CommandContractFingerprint,
    pub(super) input_hash: CanonicalInputHash,
    pub(super) causation_id: CausationId,
    pub(super) attempt_token: AttemptToken,
    pub(super) attempt_number: u64,
}

impl CommandAttempt {
    pub(crate) fn key(&self) -> &CommandLedgerKey {
        &self.key
    }

    pub(crate) fn causation_id(&self) -> &CausationId {
        &self.causation_id
    }

    #[cfg(test)]
    pub(crate) fn attempt_token(&self) -> &AttemptToken {
        &self.attempt_token
    }

    #[cfg(test)]
    pub(crate) fn attempt_number(&self) -> u64 {
        self.attempt_number
    }

    /// Cloneable, read-only generation fence retained across a consuming
    /// causal commit. If commit acknowledgement is ambiguous, the dispatcher
    /// can look up the command and mark only this exact still-live attempt as
    /// retryable-unknown; it never needs to reconstruct secret fence material
    /// from strings.
    pub(crate) fn fence(&self) -> AttemptFence {
        AttemptFence {
            key: self.key.clone(),
            contract_fingerprint: self.contract_fingerprint,
            input_hash: self.input_hash,
            causation_id: self.causation_id.clone(),
            attempt_token: self.attempt_token.clone(),
            attempt_number: self.attempt_number,
        }
    }

    pub(crate) fn complete(
        self,
        state: TerminalCommandState,
        outcome: Value,
        retention: Duration,
    ) -> Result<CommandCompletion, CommandLedgerError> {
        self.complete_with_replay_metadata(state, outcome, Vec::new(), None, retention, None)
    }

    pub(crate) fn complete_with_obligations(
        self,
        state: TerminalCommandState,
        outcome: Value,
        projection_obligations: Vec<ResolvedProjectionObligation>,
        retention: Duration,
    ) -> Result<CommandCompletion, CommandLedgerError> {
        self.complete_with_replay_metadata(
            state,
            outcome,
            projection_obligations,
            None,
            retention,
            None,
        )
    }

    /// Complete a command with exact already-canonical role-safe projection
    /// metadata.
    ///
    /// The ledger intentionally treats the bytes as opaque. GraphQL validates
    /// the versioned delta/obligation schema before calling this method; the
    /// ledger preserves those bytes exactly and applies only generic size and
    /// state invariants.
    pub(crate) fn complete_with_projection_metadata(
        self,
        state: TerminalCommandState,
        outcome: Value,
        projection_metadata: Vec<u8>,
        retention: Duration,
    ) -> Result<CommandCompletion, CommandLedgerError> {
        self.complete_with_replay_metadata(
            state,
            outcome,
            Vec::new(),
            Some(projection_metadata),
            retention,
            None,
        )
    }

    /// Complete a modeled command with the exact absolute retention boundary
    /// sealed into its authenticated projection metadata.
    ///
    /// The repository persists this same deadline atomically with the replay
    /// bytes, preventing a retained ledger tail whose metadata has already
    /// expired.
    pub(crate) fn complete_with_projection_metadata_until(
        self,
        state: TerminalCommandState,
        outcome: Value,
        projection_metadata: Vec<u8>,
        retention: Duration,
        retention_expires_at: SystemTime,
    ) -> Result<CommandCompletion, CommandLedgerError> {
        self.complete_with_replay_metadata(
            state,
            outcome,
            Vec::new(),
            Some(projection_metadata),
            retention,
            Some(retention_expires_at),
        )
    }

    fn complete_with_replay_metadata(
        self,
        state: TerminalCommandState,
        outcome: Value,
        projection_obligations: Vec<ResolvedProjectionObligation>,
        projection_metadata: Option<Vec<u8>>,
        retention: Duration,
        retention_expires_at: Option<SystemTime>,
    ) -> Result<CommandCompletion, CommandLedgerError> {
        validate_positive_duration("command replay retention", retention)?;
        if projection_metadata.is_none() {
            validate_projection_obligation_semantics(state.into(), &projection_obligations)
                .map_err(CommandLedgerError::Invalid)?;
        }
        validate_projection_metadata_bytes(state.into(), projection_metadata.as_deref())?;
        if projection_metadata.is_some() && !projection_obligations.is_empty() {
            return Err(CommandLedgerError::Invalid(
                "command replay cannot mix legacy and modeled projection obligations".into(),
            ));
        }
        let mut replay = serde_json::Map::from_iter([
            (
                "version".into(),
                Value::from(u64::from(COMMAND_REPLAY_VERSION)),
            ),
            ("outcome".into(), outcome),
            (
                "projection_obligations".into(),
                serde_json::to_value(projection_obligations).map_err(|error| {
                    CommandLedgerError::Invalid(format!(
                        "command replay obligations failed to serialize before commit: {error}"
                    ))
                })?,
            ),
        ]);
        if let Some(projection_metadata) = projection_metadata {
            replay.insert(
                "projection_metadata".into(),
                Value::String(URL_SAFE_NO_PAD.encode(projection_metadata)),
            );
        }
        let replay = serde_json::to_string(&Value::Object(replay)).map_err(|error| {
            CommandLedgerError::Invalid(format!(
                "command replay serialization failed before commit: {error}"
            ))
        })?;
        Ok(CommandCompletion {
            attempt: self,
            state,
            replay,
            direct_projection: None,
            retention,
            retention_expires_at,
        })
    }
}

/// Read-only snapshot of one attempt generation, safe to retain while the
/// owned [`CommandAttempt`] is consumed by completion.
#[derive(Clone, Debug)]
pub(crate) struct AttemptFence {
    pub(super) key: CommandLedgerKey,
    pub(super) contract_fingerprint: CommandContractFingerprint,
    pub(super) input_hash: CanonicalInputHash,
    pub(super) causation_id: CausationId,
    pub(super) attempt_token: AttemptToken,
    pub(super) attempt_number: u64,
}

impl AttemptFence {
    pub(crate) fn key(&self) -> &CommandLedgerKey {
        &self.key
    }

    pub(crate) fn causation_id(&self) -> &CausationId {
        &self.causation_id
    }

    pub(crate) fn attempt_token(&self) -> &AttemptToken {
        &self.attempt_token
    }

    pub(crate) fn attempt_number(&self) -> u64 {
        self.attempt_number
    }

    pub(crate) fn contract_fingerprint_bytes(&self) -> &[u8; SHA256_BYTES] {
        self.contract_fingerprint.as_bytes()
    }

    pub(crate) fn input_hash_bytes(&self) -> &[u8; SHA256_BYTES] {
        self.input_hash.as_bytes()
    }
}

/// Authorization scope for a command-ledger lookup.
///
/// Public status reads are bound to the selected command route. Ambiguous
/// commit recovery instead presents the private attempt fence retained by the
/// dispatcher, so it can recover a terminal outcome without trusting a route
/// name supplied by the caller.
#[derive(Clone, Copy, Debug)]
pub(crate) enum CommandLookupScope<'a> {
    CommandName(&'a str),
    CommandContract {
        command_name: &'a str,
        contract_fingerprint: &'a [u8; SHA256_BYTES],
    },
    Attempt(&'a AttemptFence),
}

impl fmt::Debug for CommandAttempt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CommandAttempt")
            .field("key", &self.key)
            .field("causation_id", &self.causation_id)
            .field("attempt_token", &"[redacted]")
            .field("attempt_number", &self.attempt_number)
            .finish()
    }
}

/// A terminal command replay recovered without invoking application code.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CommandReplay {
    pub(crate) command_id: CommandId,
    pub(crate) state: CommandLedgerState,
    pub(crate) causation_id: CausationId,
    pub(crate) outcome: Value,
    pub(crate) projection_obligations: Vec<ResolvedProjectionObligation>,
    /// Exact canonical role-safe delta and opaque obligation bytes.
    pub(crate) projection_metadata: Option<Vec<u8>>,
    /// Exact original direct-projection revision/change evidence.
    ///
    /// This is intentionally retained as the validated canonical replay value:
    /// clients replay the original command outcome, while framework recovery
    /// and diagnostics can prove which record version that transaction minted.
    pub(crate) direct_projection: Option<Value>,
}

pub(super) fn validate_projection_metadata_bytes(
    state: CommandLedgerState,
    projection_metadata: Option<&[u8]>,
) -> Result<(), CommandLedgerError> {
    let Some(bytes) = projection_metadata else {
        return Ok(());
    };
    if bytes.is_empty() || bytes.len() > crate::MAX_DOMAIN_EVENT_BODY_BYTES {
        return Err(CommandLedgerError::Invalid(format!(
            "command projection metadata must contain 1..={} bytes",
            crate::MAX_DOMAIN_EVENT_BODY_BYTES
        )));
    }
    if !matches!(
        state,
        CommandLedgerState::Succeeded
            | CommandLedgerState::SucceededPendingProjection
            | CommandLedgerState::ProjectionFailed
    ) {
        return Err(CommandLedgerError::Invalid(format!(
            "state `{}` cannot contain modeled projection metadata",
            state.as_str()
        )));
    }
    Ok(())
}

/// Result of one short reservation transaction.
#[derive(Debug)]
pub(crate) enum ReservationOutcome {
    Acquired(CommandAttempt),
    InProgress {
        /// Retained for the public receipt/status envelope added by the causal
        /// protocol layer; private dispatch currently exposes only the state.
        #[allow(dead_code)]
        causation_id: CausationId,
    },
    Replay(CommandReplay),
    Conflict,
    Expired,
}

/// Authorized status lookup result. Grant checks occur above this private
/// storage layer; a raw command ID alone never reaches this API.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum CommandLookup {
    InProgress { causation_id: CausationId },
    RetryableUnknown { causation_id: CausationId },
    Replay(CommandReplay),
    Expired,
    Unknown,
}

/// Exactly one fenced ledger completion attached to a domain commit.
pub(crate) struct CommandCompletion {
    pub(super) attempt: CommandAttempt,
    pub(super) state: TerminalCommandState,
    pub(super) replay: String,
    direct_projection: Option<Value>,
    pub(super) retention: Duration,
    retention_expires_at: Option<SystemTime>,
}

impl CommandCompletion {
    pub(crate) fn attempt(&self) -> &CommandAttempt {
        &self.attempt
    }

    pub(crate) fn state(&self) -> TerminalCommandState {
        self.state
    }

    pub(crate) fn replay_json(&self) -> &str {
        &self.replay
    }

    pub(crate) fn retention(&self) -> Duration {
        self.retention
    }

    pub(crate) fn retention_expires_at(&self) -> Option<SystemTime> {
        self.retention_expires_at
    }

    pub(crate) fn attempt_fence(&self) -> AttemptFence {
        self.attempt.fence()
    }

    /// Attach adapter-allocated same-transaction projection evidence before
    /// the ledger row is completed in that same transaction.
    pub(crate) fn attach_direct_projection(
        &mut self,
        evidence: &SameTransactionProjectionEvidence,
    ) -> Result<(), CommandLedgerError> {
        if self.state != TerminalCommandState::Projected {
            return Err(CommandLedgerError::Invalid(
                "direct projection evidence may only complete a projected command".into(),
            ));
        }
        if self.direct_projection.is_some() {
            return Err(CommandLedgerError::Invalid(
                "command completion already contains direct projection evidence".into(),
            ));
        }
        let direct_projection = evidence.replay_value();
        SameTransactionProjectionEvidence::validate_replay_value(&direct_projection)
            .map_err(CommandLedgerError::Invalid)?;

        let replay: Value = serde_json::from_str(&self.replay).map_err(|error| {
            CommandLedgerError::Invalid(format!(
                "command replay could not be extended with direct projection evidence: {error}"
            ))
        })?;
        let Value::Object(mut replay) = replay else {
            return Err(CommandLedgerError::Invalid(
                "command replay envelope is not an object".into(),
            ));
        };
        if replay
            .insert("direct_projection".into(), direct_projection.clone())
            .is_some()
        {
            return Err(CommandLedgerError::Invalid(
                "command replay already has a direct projection field".into(),
            ));
        }
        self.replay = serde_json::to_string(&replay).map_err(|error| {
            CommandLedgerError::Invalid(format!(
                "command replay serialization failed after direct projection: {error}"
            ))
        })?;
        self.direct_projection = Some(direct_projection);
        Ok(())
    }

    pub(super) fn validate_direct_projection(&self) -> Result<(), CommandLedgerError> {
        match (self.state, self.direct_projection.is_some()) {
            (TerminalCommandState::Projected, true)
            | (
                TerminalCommandState::Succeeded
                | TerminalCommandState::SucceededPendingProjection
                | TerminalCommandState::Rejected,
                false,
            ) => Ok(()),
            (TerminalCommandState::Projected, false) => Err(CommandLedgerError::Invalid(
                "projected command completion has no exact direct projection evidence".into(),
            )),
            (_, true) => Err(CommandLedgerError::Invalid(
                "non-projected command completion contains direct projection evidence".into(),
            )),
        }
    }
}

/// Existing public domain batch plus exactly one private ledger completion.
pub(crate) struct CausalCommitBatch<'a> {
    pub(crate) domain: CommitBatch<'a>,
    pub(crate) completion: CommandCompletion,
    pub(crate) direct_projection: Option<SameTransactionProjectionBatch>,
}

impl<'a> CausalCommitBatch<'a> {
    pub(crate) fn new(domain: CommitBatch<'a>, completion: CommandCompletion) -> Self {
        Self::build(domain, completion, None)
    }

    pub(crate) fn with_direct_projection(
        domain: CommitBatch<'a>,
        completion: CommandCompletion,
        direct_projection: SameTransactionProjectionBatch,
    ) -> Self {
        Self::build(domain, completion, Some(direct_projection))
    }

    fn build(
        mut domain: CommitBatch<'a>,
        completion: CommandCompletion,
        direct_projection: Option<SameTransactionProjectionBatch>,
    ) -> Self {
        // The attempt's stable causation is authoritative at the final boundary:
        // handler metadata cannot accidentally (or deliberately) split the
        // event/outbox effects from their durable command identity.
        let causation_id = completion.attempt().causation_id().as_str();
        for stream in &mut domain.streams {
            stream.entity.overwrite_new_event_causation_id(causation_id);
        }
        for message in &mut domain.outbox_messages {
            message.overwrite_causation_id(causation_id);
        }
        Self {
            domain,
            completion,
            direct_projection,
        }
    }
}
