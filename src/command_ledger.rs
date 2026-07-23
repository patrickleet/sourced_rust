//! Adapter-neutral durable command identity and fenced commit primitives.
//!
//! This module is deliberately crate-private. Application handlers interact
//! with the typed command API; only the framework dispatcher and repository
//! adapters may reserve attempts or attach a ledger completion to a domain
//! [`CommitBatch`]. Keeping the completion outside the public batch preserves
//! the existing repository API while making causal completion inseparable from
//! the backend transaction that stores the domain effects.

#![cfg_attr(not(feature = "graphql"), allow(dead_code))]

use std::collections::HashSet;
use std::fmt;
use std::future::Future;
use std::time::{Duration, SystemTime};

use serde_json::Value;
use uuid::{Uuid, Variant};

use crate::entity::Entity;
use crate::graphql::command_contract::ResolvedProjectionObligation;
use crate::repository::{CommitBatch, RepositoryError, StreamIdentity};

const SHA256_BYTES: usize = 32;
const COMMAND_REPLAY_VERSION: u16 = 1;

/// Opaque identity for one concrete leaf repository instance.
///
/// Clones copy this value. Constructing a new leaf repository over even the
/// same underlying pool creates a different identity, so GraphQL command
/// binding must retain the repository handle that owns the causal committer.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct CausalStorageIdentity(Uuid);

impl CausalStorageIdentity {
    pub(crate) fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl fmt::Debug for CausalStorageIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CausalStorageIdentity([opaque])")
    }
}

/// Canonical client-created UUIDv7 command identity.
#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct CommandId(String);

impl CommandId {
    pub(crate) fn parse(value: impl AsRef<str>) -> Result<Self, CommandLedgerError> {
        let value = value.as_ref();
        let parsed = Uuid::parse_str(value).map_err(|_| {
            CommandLedgerError::Invalid(format!("command ID `{value}` must be a valid UUIDv7"))
        })?;
        if parsed.get_version_num() != 7 || parsed.get_variant() != Variant::RFC4122 {
            return Err(CommandLedgerError::Invalid(format!(
                "command ID `{value}` must be an RFC 4122 UUIDv7"
            )));
        }
        Ok(Self(parsed.hyphenated().to_string()))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for CommandId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("CommandId").field(&self.0).finish()
    }
}

/// Versioned server-derived verified-principal partition.
#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct PrincipalPartitionId(String);

impl PrincipalPartitionId {
    pub(crate) fn new(value: impl Into<String>) -> Result<Self, CommandLedgerError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(CommandLedgerError::Invalid(
                "principal partition must not be empty".into(),
            ));
        }
        Ok(Self(value))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for PrincipalPartitionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PrincipalPartitionId([redacted])")
    }
}

/// Complete non-forgeable ledger key.
#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct CommandLedgerKey {
    service_id: String,
    principal_partition: PrincipalPartitionId,
    command_id: CommandId,
}

impl CommandLedgerKey {
    pub(crate) fn new(
        service_id: impl Into<String>,
        principal_partition: PrincipalPartitionId,
        command_id: CommandId,
    ) -> Result<Self, CommandLedgerError> {
        let service_id = service_id.into();
        if service_id.trim().is_empty() {
            return Err(CommandLedgerError::Invalid(
                "command ledger service ID must not be empty".into(),
            ));
        }
        Ok(Self {
            service_id,
            principal_partition,
            command_id,
        })
    }

    pub(crate) fn service_id(&self) -> &str {
        &self.service_id
    }

    pub(crate) fn principal_partition(&self) -> &str {
        self.principal_partition.as_str()
    }

    pub(crate) fn command_id(&self) -> &str {
        self.command_id.as_str()
    }
}

impl fmt::Debug for CommandLedgerKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CommandLedgerKey")
            .field("service_id", &self.service_id)
            .field("principal_partition", &"[redacted]")
            .field("command_id", &self.command_id)
            .finish()
    }
}

macro_rules! fixed_hash {
    ($name:ident, $description:literal) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub(crate) struct $name([u8; SHA256_BYTES]);

        impl $name {
            pub(crate) fn new(bytes: [u8; SHA256_BYTES]) -> Self {
                Self(bytes)
            }

            pub(crate) fn try_from_slice(bytes: &[u8]) -> Result<Self, CommandLedgerError> {
                let bytes: [u8; SHA256_BYTES] = bytes.try_into().map_err(|_| {
                    CommandLedgerError::Invalid(format!(
                        "{} must contain exactly {SHA256_BYTES} bytes",
                        $description
                    ))
                })?;
                Ok(Self(bytes))
            }

            /// Parse the canonical wire spelling emitted by command-input and
            /// command-contract code (`sha256:` followed by 64 lowercase hex
            /// digits). Keeping this checked seam here prevents dispatch code
            /// from hand-decoding identity material differently at each call
            /// site.
            #[cfg(test)]
            pub(crate) fn parse_sha256(value: &str) -> Result<Self, CommandLedgerError> {
                parse_sha256(value, $description).map(Self)
            }

            pub(crate) fn as_bytes(&self) -> &[u8; SHA256_BYTES] {
                &self.0
            }
        }
    };
}

fixed_hash!(CanonicalInputHash, "canonical command input hash");
fixed_hash!(CommandContractFingerprint, "command contract fingerprint");

#[cfg(test)]
fn parse_sha256(
    value: &str,
    description: &'static str,
) -> Result<[u8; SHA256_BYTES], CommandLedgerError> {
    let encoded = value.strip_prefix("sha256:").ok_or_else(|| {
        CommandLedgerError::Invalid(format!(
            "{description} must use the canonical `sha256:<lowercase-hex>` format"
        ))
    })?;
    if encoded.len() != SHA256_BYTES * 2 {
        return Err(CommandLedgerError::Invalid(format!(
            "{description} must contain exactly {} hexadecimal digits",
            SHA256_BYTES * 2
        )));
    }

    fn nibble(byte: u8) -> Option<u8> {
        match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            _ => None,
        }
    }

    let encoded = encoded.as_bytes();
    let mut digest = [0; SHA256_BYTES];
    for (index, target) in digest.iter_mut().enumerate() {
        let high = nibble(encoded[index * 2]);
        let low = nibble(encoded[index * 2 + 1]);
        let (Some(high), Some(low)) = (high, low) else {
            return Err(CommandLedgerError::Invalid(format!(
                "{description} must contain only lowercase hexadecimal digits"
            )));
        };
        *target = (high << 4) | low;
    }
    Ok(digest)
}

/// Stable causation allocated exactly once when a ledger identity is inserted.
#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct CausationId(String);

impl CausationId {
    pub(crate) fn new() -> Self {
        Self(Uuid::now_v7().hyphenated().to_string())
    }

    pub(crate) fn parse_stored(value: String) -> Result<Self, CommandLedgerError> {
        parse_stored_uuid_v7("causation ID", value).map(Self)
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for CausationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("CausationId").field(&self.0).finish()
    }
}

/// Generation fence for one speculative handler attempt.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct AttemptToken(String);

impl AttemptToken {
    fn new() -> Self {
        Self(Uuid::now_v7().hyphenated().to_string())
    }

    pub(crate) fn parse_stored(value: String) -> Result<Self, CommandLedgerError> {
        parse_stored_uuid_v7("attempt token", value).map(Self)
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for AttemptToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AttemptToken([redacted])")
    }
}

fn parse_stored_uuid_v7(label: &str, value: String) -> Result<String, CommandLedgerError> {
    let parsed = Uuid::parse_str(&value)
        .map_err(|_| CommandLedgerError::Corrupt(format!("stored {label} is not a UUID")))?;
    if parsed.get_version_num() != 7 || parsed.get_variant() != Variant::RFC4122 {
        return Err(CommandLedgerError::Corrupt(format!(
            "stored {label} is not an RFC 4122 UUIDv7"
        )));
    }
    Ok(parsed.hyphenated().to_string())
}

/// Durable command lifecycle. `Unknown` is intentionally not stored: absence
/// is represented by [`CommandLookup::Unknown`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CommandLedgerState {
    InProgress,
    RetryableUnknown,
    Accepted,
    AcceptedPendingProjection,
    Projected,
    Rejected,
    ProjectionFailed,
    Expired,
}

impl CommandLedgerState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::InProgress => "in_progress",
            Self::RetryableUnknown => "retryable_unknown",
            Self::Accepted => "accepted",
            Self::AcceptedPendingProjection => "accepted_pending_projection",
            Self::Projected => "projected",
            Self::Rejected => "rejected",
            Self::ProjectionFailed => "projection_failed",
            Self::Expired => "expired",
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self, CommandLedgerError> {
        match value {
            "in_progress" => Ok(Self::InProgress),
            "retryable_unknown" => Ok(Self::RetryableUnknown),
            "accepted" => Ok(Self::Accepted),
            "accepted_pending_projection" => Ok(Self::AcceptedPendingProjection),
            "projected" => Ok(Self::Projected),
            "rejected" => Ok(Self::Rejected),
            "projection_failed" => Ok(Self::ProjectionFailed),
            "expired" => Ok(Self::Expired),
            other => Err(CommandLedgerError::Corrupt(format!(
                "stored command ledger state `{other}` is invalid"
            ))),
        }
    }

    fn is_replayable(self) -> bool {
        matches!(
            self,
            Self::Accepted
                | Self::AcceptedPendingProjection
                | Self::Projected
                | Self::Rejected
                | Self::ProjectionFailed
        )
    }
}

/// States the command dispatcher may commit through an attempt fence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TerminalCommandState {
    Accepted,
    AcceptedPendingProjection,
    Projected,
    Rejected,
}

impl From<TerminalCommandState> for CommandLedgerState {
    fn from(value: TerminalCommandState) -> Self {
        match value {
            TerminalCommandState::Accepted => Self::Accepted,
            TerminalCommandState::AcceptedPendingProjection => Self::AcceptedPendingProjection,
            TerminalCommandState::Projected => Self::Projected,
            TerminalCommandState::Rejected => Self::Rejected,
        }
    }
}

fn validate_projection_obligation_semantics(
    state: CommandLedgerState,
    obligations: &[ResolvedProjectionObligation],
) -> Result<(), String> {
    match state {
        CommandLedgerState::Accepted
        | CommandLedgerState::Projected
        | CommandLedgerState::Rejected => {
            if !obligations.is_empty() {
                return Err(format!(
                    "state `{}` must not contain projection obligations",
                    state.as_str()
                ));
            }
        }
        CommandLedgerState::AcceptedPendingProjection | CommandLedgerState::ProjectionFailed => {
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

/// One validated reservation request. Fresh candidate IDs lose a race safely:
/// only the inserted row keeps them; every retry reads the winner's causation.
pub(crate) struct CommandReservation {
    key: CommandLedgerKey,
    command_name: String,
    contract_fingerprint: CommandContractFingerprint,
    input_hash: CanonicalInputHash,
    lease: Duration,
    retention: Duration,
    candidate_causation: CausationId,
    candidate_attempt: AttemptToken,
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
    key: CommandLedgerKey,
    contract_fingerprint: CommandContractFingerprint,
    input_hash: CanonicalInputHash,
    causation_id: CausationId,
    attempt_token: AttemptToken,
    attempt_number: u64,
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
        self.complete_with_obligations(state, outcome, Vec::new(), retention)
    }

    pub(crate) fn complete_with_obligations(
        self,
        state: TerminalCommandState,
        outcome: Value,
        projection_obligations: Vec<ResolvedProjectionObligation>,
        retention: Duration,
    ) -> Result<CommandCompletion, CommandLedgerError> {
        validate_positive_duration("command replay retention", retention)?;
        validate_projection_obligation_semantics(state.into(), &projection_obligations)
            .map_err(CommandLedgerError::Invalid)?;
        let replay = serde_json::to_string(&serde_json::json!({
            "version": COMMAND_REPLAY_VERSION,
            "outcome": outcome,
            "projection_obligations": projection_obligations,
        }))
        .map_err(|error| {
            CommandLedgerError::Invalid(format!(
                "command replay serialization failed before commit: {error}"
            ))
        })?;
        Ok(CommandCompletion {
            attempt: self,
            state,
            replay,
            retention,
        })
    }
}

/// Read-only snapshot of one attempt generation, safe to retain while the
/// owned [`CommandAttempt`] is consumed by completion.
#[derive(Clone, Debug)]
pub(crate) struct AttemptFence {
    key: CommandLedgerKey,
    contract_fingerprint: CommandContractFingerprint,
    input_hash: CanonicalInputHash,
    causation_id: CausationId,
    attempt_token: AttemptToken,
    attempt_number: u64,
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
    pub(crate) state: CommandLedgerState,
    pub(crate) causation_id: CausationId,
    pub(crate) outcome: Value,
    pub(crate) projection_obligations: Vec<ResolvedProjectionObligation>,
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
    attempt: CommandAttempt,
    state: TerminalCommandState,
    replay: String,
    retention: Duration,
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

    pub(crate) fn attempt_fence(&self) -> AttemptFence {
        self.attempt.fence()
    }
}

/// Existing public domain batch plus exactly one private ledger completion.
pub(crate) struct CausalCommitBatch<'a> {
    pub(crate) domain: CommitBatch<'a>,
    pub(crate) completion: CommandCompletion,
}

impl<'a> CausalCommitBatch<'a> {
    pub(crate) fn new(mut domain: CommitBatch<'a>, completion: CommandCompletion) -> Self {
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
        Self { domain, completion }
    }
}

/// Read capability used by the causal workspace. Unlike ordinary
/// `QueuedRepository::get_stream`, wrapper implementations must not retain a
/// queue lock while user handler code awaits.
pub(crate) trait CausalGetStream: Send + Sync {
    fn get_causal_stream<'a>(
        &'a self,
        identity: &'a StreamIdentity,
    ) -> impl Future<Output = Result<Option<Entity>, RepositoryError>> + Send + 'a;
}

/// Proves that command reservation, stream loading, and causal commit are all
/// routed to the same concrete leaf repository handle. Wrappers delegate the
/// opaque value; independently constructed leaves always mint a new one.
pub(crate) trait CausalRepositoryIdentity: Send + Sync {
    fn causal_storage_identity(&self) -> CausalStorageIdentity;
}

/// Short-transaction ledger operations. Lease recovery is part of `reserve` so
/// an adapter cannot expose a non-atomic read-then-steal sequence.
pub(crate) trait CommandLedgerStore: Send + Sync {
    fn reserve_command(
        &self,
        reservation: CommandReservation,
    ) -> impl Future<Output = Result<ReservationOutcome, CommandLedgerError>> + Send + '_;

    fn lookup_command<'a>(
        &'a self,
        key: &'a CommandLedgerKey,
        scope: CommandLookupScope<'a>,
    ) -> impl Future<Output = Result<CommandLookup, CommandLedgerError>> + Send + 'a;

    fn mark_retryable_unknown(
        &self,
        attempt: AttemptFence,
    ) -> impl Future<Output = Result<(), CommandLedgerError>> + Send + '_;

    #[allow(dead_code)]
    fn compact_expired_commands(
        &self,
        limit: usize,
    ) -> impl Future<Output = Result<u64, CommandLedgerError>> + Send + '_;
}

/// Private transaction capability that makes terminal ledger completion an
/// inseparable participant in the domain commit.
pub(crate) trait CausalTransactionalCommit: Send + Sync {
    fn commit_causal_batch<'a>(
        &'a self,
        batch: CausalCommitBatch<'a>,
    ) -> impl Future<Output = Result<(), CommandLedgerError>> + Send + 'a;
}

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

/// Storage-neutral row representation shared by built-in adapters.
#[derive(Clone, Debug)]
pub(crate) struct CommandLedgerRecord {
    pub(crate) key: CommandLedgerKey,
    pub(crate) command_name: String,
    pub(crate) contract_fingerprint: CommandContractFingerprint,
    pub(crate) input_hash: CanonicalInputHash,
    pub(crate) state: CommandLedgerState,
    pub(crate) causation_id: CausationId,
    pub(crate) attempt_token: Option<AttemptToken>,
    pub(crate) attempt_number: u64,
    pub(crate) lease_expires_at: Option<SystemTime>,
    pub(crate) outcome_json: Option<String>,
    #[allow(dead_code)]
    pub(crate) created_at: SystemTime,
    pub(crate) updated_at: SystemTime,
    pub(crate) completed_at: Option<SystemTime>,
    pub(crate) retention_expires_at: SystemTime,
    pub(crate) compacted_at: Option<SystemTime>,
}

impl CommandLedgerRecord {
    pub(crate) fn initial(
        reservation: &CommandReservation,
        now: SystemTime,
    ) -> Result<Self, CommandLedgerError> {
        Ok(Self {
            key: reservation.key.clone(),
            command_name: reservation.command_name.clone(),
            contract_fingerprint: reservation.contract_fingerprint,
            input_hash: reservation.input_hash,
            state: CommandLedgerState::InProgress,
            causation_id: reservation.candidate_causation.clone(),
            attempt_token: Some(AttemptToken(reservation.candidate_attempt.0.clone())),
            attempt_number: 1,
            lease_expires_at: Some(checked_deadline(now, reservation.lease, "attempt lease")?),
            outcome_json: None,
            created_at: now,
            updated_at: now,
            completed_at: None,
            retention_expires_at: checked_deadline(
                now,
                reservation.retention,
                "command retention",
            )?,
            compacted_at: None,
        })
    }

    pub(crate) fn acquired_attempt(&self) -> Result<CommandAttempt, CommandLedgerError> {
        let token = self.attempt_token.as_ref().ok_or_else(|| {
            CommandLedgerError::Corrupt(format!(
                "in-progress command `{}` has no attempt token",
                self.key.command_id()
            ))
        })?;
        Ok(CommandAttempt {
            key: self.key.clone(),
            contract_fingerprint: self.contract_fingerprint,
            input_hash: self.input_hash,
            causation_id: self.causation_id.clone(),
            attempt_token: AttemptToken(token.0.clone()),
            attempt_number: self.attempt_number,
        })
    }

    pub(crate) fn classify_reservation(
        &self,
        reservation: &CommandReservation,
        now: SystemTime,
    ) -> Result<ReservationDecision, CommandLedgerError> {
        if self.state == CommandLedgerState::Expired || self.retention_expires_at <= now {
            return Ok(ReservationDecision::Expire);
        }
        if self.command_name != reservation.command_name
            || self.contract_fingerprint != reservation.contract_fingerprint
            || self.input_hash != reservation.input_hash
        {
            return Ok(ReservationDecision::Conflict);
        }
        match self.state {
            CommandLedgerState::InProgress => {
                let lease = self.lease_expires_at.ok_or_else(|| {
                    CommandLedgerError::Corrupt(format!(
                        "in-progress command `{}` has no lease deadline",
                        self.key.command_id()
                    ))
                })?;
                if lease <= now {
                    Ok(ReservationDecision::Reclaim)
                } else {
                    Ok(ReservationDecision::InProgress)
                }
            }
            CommandLedgerState::RetryableUnknown => Ok(ReservationDecision::Reclaim),
            state if state.is_replayable() => Ok(ReservationDecision::Replay),
            CommandLedgerState::Expired => Ok(ReservationDecision::Expire),
            other => Err(CommandLedgerError::Corrupt(format!(
                "command `{}` has unsupported state `{}`",
                self.key.command_id(),
                other.as_str()
            ))),
        }
    }

    pub(crate) fn reclaim(
        &mut self,
        reservation: &CommandReservation,
        now: SystemTime,
    ) -> Result<(), CommandLedgerError> {
        let attempt_number = self.attempt_number.checked_add(1).ok_or_else(|| {
            CommandLedgerError::Corrupt(format!(
                "command `{}` attempt counter overflowed",
                self.key.command_id()
            ))
        })?;
        let lease_expires_at = checked_deadline(now, reservation.lease, "attempt lease")?;
        let retention_expires_at =
            checked_deadline(now, reservation.retention, "command retention")?;

        self.state = CommandLedgerState::InProgress;
        self.attempt_token = Some(AttemptToken(reservation.candidate_attempt.0.clone()));
        self.attempt_number = attempt_number;
        self.lease_expires_at = Some(lease_expires_at);
        self.retention_expires_at = retention_expires_at;
        self.updated_at = now;
        Ok(())
    }

    pub(crate) fn validate_stored_shape(&self) -> Result<(), CommandLedgerError> {
        if self.command_name.trim().is_empty() || self.attempt_number == 0 {
            return Err(CommandLedgerError::Corrupt(format!(
                "command `{}` has invalid invariant fields",
                self.key.command_id()
            )));
        }
        let valid = match self.state {
            CommandLedgerState::InProgress => {
                self.attempt_token.is_some()
                    && self.lease_expires_at.is_some()
                    && self.outcome_json.is_none()
                    && self.completed_at.is_none()
                    && self.compacted_at.is_none()
            }
            CommandLedgerState::RetryableUnknown => {
                self.attempt_token.is_none()
                    && self.lease_expires_at.is_none()
                    && self.outcome_json.is_none()
                    && self.completed_at.is_none()
                    && self.compacted_at.is_none()
            }
            state if state.is_replayable() => {
                self.attempt_token.is_none()
                    && self.lease_expires_at.is_none()
                    && self.outcome_json.is_some()
                    && self.completed_at.is_some()
                    && self.compacted_at.is_none()
            }
            CommandLedgerState::Expired => {
                self.attempt_token.is_none()
                    && self.lease_expires_at.is_none()
                    && self.outcome_json.is_none()
                    && self.compacted_at.is_some()
            }
            _ => false,
        };
        if !valid {
            return Err(CommandLedgerError::Corrupt(format!(
                "command `{}` state `{}` has inconsistent nullable fields",
                self.key.command_id(),
                self.state.as_str()
            )));
        }
        Ok(())
    }

    pub(crate) fn expire(&mut self, now: SystemTime) {
        self.state = CommandLedgerState::Expired;
        self.attempt_token = None;
        self.lease_expires_at = None;
        self.outcome_json = None;
        self.updated_at = now;
        self.compacted_at = Some(now);
    }

    pub(crate) fn matches_fence(&self, attempt: &AttemptFence) -> bool {
        self.key == attempt.key
            && self.contract_fingerprint == attempt.contract_fingerprint
            && self.input_hash == attempt.input_hash
            && self.state == CommandLedgerState::InProgress
            && self.causation_id == attempt.causation_id
            && self.attempt_token.as_ref() == Some(&attempt.attempt_token)
            && self.attempt_number == attempt.attempt_number
    }

    pub(crate) fn matches_lookup_scope(&self, scope: CommandLookupScope<'_>) -> bool {
        match scope {
            CommandLookupScope::CommandName(expected) => self.command_name == expected,
            CommandLookupScope::Attempt(attempt) => {
                self.key == attempt.key
                    && self.contract_fingerprint == attempt.contract_fingerprint
                    && self.input_hash == attempt.input_hash
                    && self.causation_id == attempt.causation_id
                    && self.attempt_number == attempt.attempt_number
                    && self
                        .attempt_token
                        .as_ref()
                        .is_none_or(|token| token == &attempt.attempt_token)
            }
        }
    }

    pub(crate) fn mark_retryable_unknown(
        &mut self,
        attempt: &AttemptFence,
        now: SystemTime,
    ) -> Result<(), CommandLedgerError> {
        if !self.matches_fence(attempt) {
            return Err(CommandLedgerError::AttemptFenced {
                command_id: attempt.key.command_id().to_string(),
            });
        }
        self.state = CommandLedgerState::RetryableUnknown;
        self.attempt_token = None;
        self.lease_expires_at = None;
        self.updated_at = now;
        Ok(())
    }

    pub(crate) fn complete(
        &mut self,
        completion: &CommandCompletion,
        now: SystemTime,
    ) -> Result<(), CommandLedgerError> {
        let lease_is_live = self
            .lease_expires_at
            .is_some_and(|lease_expires_at| lease_expires_at > now);
        if !self.matches_fence(&completion.attempt.fence()) || !lease_is_live {
            return Err(CommandLedgerError::AttemptFenced {
                command_id: completion.attempt.key.command_id().to_string(),
            });
        }
        let retention_expires_at =
            checked_deadline(now, completion.retention, "command retention")?;
        self.state = completion.state.into();
        self.attempt_token = None;
        self.lease_expires_at = None;
        self.outcome_json = Some(completion.replay.clone());
        self.updated_at = now;
        self.completed_at = Some(now);
        self.retention_expires_at = retention_expires_at;
        Ok(())
    }

    pub(crate) fn replay(&self) -> Result<CommandReplay, CommandLedgerError> {
        if !self.state.is_replayable() {
            return Err(CommandLedgerError::Corrupt(format!(
                "command `{}` state `{}` is not replayable",
                self.key.command_id(),
                self.state.as_str()
            )));
        }
        let payload = self.outcome_json.as_deref().ok_or_else(|| {
            CommandLedgerError::Corrupt(format!(
                "replayable command `{}` has no outcome",
                self.key.command_id()
            ))
        })?;
        let envelope: Value = serde_json::from_str(payload).map_err(|error| {
            CommandLedgerError::Corrupt(format!(
                "command `{}` outcome is invalid JSON: {error}",
                self.key.command_id()
            ))
        })?;
        let Value::Object(mut envelope) = envelope else {
            return Err(CommandLedgerError::Corrupt(format!(
                "command `{}` replay envelope is not an object",
                self.key.command_id()
            )));
        };
        let version = envelope
            .remove("version")
            .and_then(|value| value.as_u64())
            .ok_or_else(|| {
                CommandLedgerError::Corrupt(format!(
                    "command `{}` replay envelope has no numeric version",
                    self.key.command_id()
                ))
            })?;
        if version != u64::from(COMMAND_REPLAY_VERSION) {
            return Err(CommandLedgerError::Corrupt(format!(
                "command `{}` replay envelope version `{version}` is unsupported",
                self.key.command_id()
            )));
        }
        let outcome = envelope.remove("outcome").ok_or_else(|| {
            CommandLedgerError::Corrupt(format!(
                "command `{}` replay envelope has no outcome",
                self.key.command_id()
            ))
        })?;
        let projection_obligations_value =
            envelope.remove("projection_obligations").ok_or_else(|| {
                CommandLedgerError::Corrupt(format!(
                    "command `{}` replay envelope has no projection obligations",
                    self.key.command_id()
                ))
            })?;
        let projection_obligations: Vec<ResolvedProjectionObligation> =
            serde_json::from_value(projection_obligations_value.clone()).map_err(|error| {
                CommandLedgerError::Corrupt(format!(
                    "command `{}` replay projection obligations are invalid: {error}",
                    self.key.command_id()
                ))
            })?;
        let canonical_projection_obligations = serde_json::to_value(&projection_obligations)
            .map_err(|error| {
                CommandLedgerError::Corrupt(format!(
                    "command `{}` replay projection obligations cannot be normalized: {error}",
                    self.key.command_id()
                ))
            })?;
        if canonical_projection_obligations != projection_obligations_value {
            return Err(CommandLedgerError::Corrupt(format!(
                "command `{}` replay projection obligations contain unknown or non-canonical fields",
                self.key.command_id()
            )));
        }
        validate_projection_obligation_semantics(self.state, &projection_obligations).map_err(
            |error| {
                CommandLedgerError::Corrupt(format!(
                    "command `{}` replay projection obligations are inconsistent: {error}",
                    self.key.command_id()
                ))
            },
        )?;
        if !envelope.is_empty() {
            return Err(CommandLedgerError::Corrupt(format!(
                "command `{}` replay envelope has unknown fields",
                self.key.command_id()
            )));
        }
        Ok(CommandReplay {
            state: self.state,
            causation_id: self.causation_id.clone(),
            outcome,
            projection_obligations,
        })
    }

    pub(crate) fn lookup(&self) -> Result<CommandLookup, CommandLedgerError> {
        match self.state {
            CommandLedgerState::InProgress => Ok(CommandLookup::InProgress {
                causation_id: self.causation_id.clone(),
            }),
            CommandLedgerState::RetryableUnknown => Ok(CommandLookup::RetryableUnknown {
                causation_id: self.causation_id.clone(),
            }),
            CommandLedgerState::Expired => Ok(CommandLookup::Expired),
            state if state.is_replayable() => Ok(CommandLookup::Replay(self.replay()?)),
            other => Err(CommandLedgerError::Corrupt(format!(
                "command `{}` has unsupported lookup state `{}`",
                self.key.command_id(),
                other.as_str()
            ))),
        }
    }

    pub(crate) fn reservation_outcome(
        &self,
        decision: ReservationDecision,
    ) -> Result<ReservationOutcome, CommandLedgerError> {
        match decision {
            ReservationDecision::Conflict => Ok(ReservationOutcome::Conflict),
            ReservationDecision::Expire => Ok(ReservationOutcome::Expired),
            ReservationDecision::InProgress => Ok(ReservationOutcome::InProgress {
                causation_id: self.causation_id.clone(),
            }),
            ReservationDecision::Replay => Ok(ReservationOutcome::Replay(self.replay()?)),
            ReservationDecision::Reclaim => {
                Ok(ReservationOutcome::Acquired(self.acquired_attempt()?))
            }
        }
    }
}

fn checked_deadline(
    now: SystemTime,
    duration: Duration,
    label: &str,
) -> Result<SystemTime, CommandLedgerError> {
    now.checked_add(duration).ok_or_else(|| {
        CommandLedgerError::Invalid(format!("{label} deadline exceeds SystemTime range"))
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReservationDecision {
    Conflict,
    Expire,
    InProgress,
    Replay,
    Reclaim,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graphql::command_contract::{ResolvedProjectionKey, ResolvedProjectionKeyField};
    use crate::microsvc::HasOutboxStore;
    use crate::outbox::{OutboxMessage, OutboxMessageStatus};
    use crate::outbox_worker::OutboxStore;
    use crate::read_model::{ReadModelWritePlanBuilder, RelationalReadModel};
    use crate::repository::{
        GetStream, InboxReceipt, InboxStore, RelationalReadModelQueryStore, SnapshotStore,
        SnapshotWrite, StreamWrite,
    };
    use crate::snapshot::SnapshotRecord;
    use crate::table::{
        ColumnType, PrimaryKey, RowKey, RowValue, RowValues, TableColumn, TableKind, TableSchema,
        TableSchemaRegistry, TableStoreError,
    };

    #[derive(Clone, Debug)]
    struct LedgerConformanceView {
        id: String,
        marker: String,
    }

    impl RelationalReadModel for LedgerConformanceView {
        fn schema() -> &'static TableSchema {
            static SCHEMA: std::sync::LazyLock<TableSchema> =
                std::sync::LazyLock::new(|| TableSchema {
                    model_name: "LedgerConformanceView".into(),
                    table_name: "command_ledger_conformance_views".into(),
                    columns: vec![
                        TableColumn {
                            primary_key: true,
                            ..TableColumn::new("id", "id", ColumnType::Text)
                        },
                        TableColumn::new("marker", "marker", ColumnType::Text),
                    ],
                    primary_key: PrimaryKey::new(["id"]),
                    version_column: Some(crate::table::DEFAULT_TABLE_VERSION_COLUMN.into()),
                    foreign_keys: Vec::new(),
                    indexes: Vec::new(),
                    relationships: Vec::new(),
                    kind: TableKind::ReadModel,
                });
            &SCHEMA
        }

        fn primary_key(&self) -> Result<RowKey, TableStoreError> {
            Ok(RowKey::new([("id", RowValue::String(self.id.clone()))]))
        }

        fn to_row(&self) -> Result<RowValues, TableStoreError> {
            let mut row = RowValues::new();
            row.insert("id", RowValue::String(self.id.clone()));
            row.insert("marker", RowValue::String(self.marker.clone()));
            Ok(row)
        }

        fn from_row(row: RowValues) -> Result<Self, TableStoreError> {
            Ok(Self {
                id: row.get_serde("id")?,
                marker: row.get_serde("marker")?,
            })
        }
    }

    fn conformance_table_registry() -> TableSchemaRegistry {
        let mut registry = TableSchemaRegistry::new();
        registry
            .register::<LedgerConformanceView>()
            .expect("ledger conformance schema should register");
        registry
    }

    fn resolved_obligation(marker: &str) -> ResolvedProjectionObligation {
        ResolvedProjectionObligation {
            projector: format!("projector-{marker}"),
            model: "LedgerConformanceView".into(),
            key: ResolvedProjectionKey {
                fields: vec![ResolvedProjectionKeyField {
                    field: "id".into(),
                    value: serde_json::json!({"wire": marker, "wide": "18446744073709551615"}),
                }],
            },
            partition: Some(serde_json::Value::Null),
        }
    }

    fn fresh_attempt() -> CommandAttempt {
        let request = reservation(&Uuid::now_v7().to_string(), 1, 2).unwrap();
        CommandLedgerRecord::initial(&request, SystemTime::now())
            .unwrap()
            .acquired_attempt()
            .unwrap()
    }

    fn completed_replay_record(
        state: CommandLedgerState,
        obligations: Vec<ResolvedProjectionObligation>,
    ) -> CommandLedgerRecord {
        let request = reservation(&Uuid::now_v7().to_string(), 1, 2).unwrap();
        let started = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
        let mut row = CommandLedgerRecord::initial(&request, started).unwrap();
        row.state = state;
        row.attempt_token = None;
        row.lease_expires_at = None;
        row.outcome_json = Some(
            serde_json::json!({
                "version": COMMAND_REPLAY_VERSION,
                "outcome": {"ok": true},
                "projection_obligations": obligations,
            })
            .to_string(),
        );
        row.updated_at = started + Duration::from_secs(1);
        row.completed_at = Some(started + Duration::from_secs(1));
        row
    }

    fn reservation_for_partition(
        command_id: &str,
        principal_partition: &str,
        command_name: &str,
        contract: u8,
        input: u8,
    ) -> Result<CommandReservation, CommandLedgerError> {
        reservation_for_partition_with_policy(
            command_id,
            principal_partition,
            command_name,
            contract,
            input,
            Duration::from_secs(30),
            Duration::from_secs(300),
        )
    }

    fn reservation_for_partition_with_policy(
        command_id: &str,
        principal_partition: &str,
        command_name: &str,
        contract: u8,
        input: u8,
        lease: Duration,
        retention: Duration,
    ) -> Result<CommandReservation, CommandLedgerError> {
        CommandReservation::new(
            CommandLedgerKey::new(
                "orders",
                PrincipalPartitionId::new(principal_partition)?,
                CommandId::parse(command_id)?,
            )?,
            command_name,
            CommandContractFingerprint::new([contract; 32]),
            CanonicalInputHash::new([input; 32]),
            lease,
            retention,
        )
    }

    fn reservation(
        command_id: &str,
        contract: u8,
        input: u8,
    ) -> Result<CommandReservation, CommandLedgerError> {
        reservation_for_partition(
            command_id,
            "v1:sha256:principal",
            "order.create",
            contract,
            input,
        )
    }

    trait CommandLedgerAdapterConformance:
        CommandLedgerStore
        + CausalTransactionalCommit
        + GetStream
        + SnapshotStore
        + InboxStore
        + RelationalReadModelQueryStore
        + HasOutboxStore
    {
    }

    impl<T> CommandLedgerAdapterConformance for T where
        T: CommandLedgerStore
            + CausalTransactionalCommit
            + GetStream
            + SnapshotStore
            + InboxStore
            + RelationalReadModelQueryStore
            + HasOutboxStore
    {
    }

    async fn acquire<R>(repo: &R, request: CommandReservation) -> CommandAttempt
    where
        R: CommandLedgerStore,
    {
        match repo.reserve_command(request).await.unwrap() {
            ReservationOutcome::Acquired(attempt) => attempt,
            other => panic!("expected acquired command attempt, got {other:?}"),
        }
    }

    async fn same_input_retries_and_identity_conflicts_conform<R>(repo: &R)
    where
        R: CommandLedgerAdapterConformance,
    {
        let id = Uuid::now_v7().to_string();
        let request = reservation(&id, 11, 12).unwrap();
        let key = request.key().clone();
        let attempt = acquire(repo, request).await;
        let causation = attempt.causation_id().clone();

        match repo
            .reserve_command(reservation(&id, 11, 12).unwrap())
            .await
            .unwrap()
        {
            ReservationOutcome::InProgress { causation_id } => {
                assert_eq!(causation_id, causation)
            }
            other => panic!("same-input retry should remain in progress, got {other:?}"),
        }
        assert!(matches!(
            repo.reserve_command(reservation(&id, 11, 99).unwrap())
                .await
                .unwrap(),
            ReservationOutcome::Conflict
        ));
        assert!(matches!(
            repo.reserve_command(
                reservation_for_partition(&id, "v1:sha256:principal", "order.cancel", 11, 12,)
                    .unwrap(),
            )
            .await
            .unwrap(),
            ReservationOutcome::Conflict
        ));

        let expected_outcome = serde_json::json!({"order_id": "same-input"});
        let completion = attempt
            .complete(
                TerminalCommandState::Accepted,
                expected_outcome.clone(),
                Duration::from_secs(300),
            )
            .unwrap();
        repo.commit_causal_batch(CausalCommitBatch::new(CommitBatch::empty(), completion))
            .await
            .unwrap();

        let reserved_replay = match repo
            .reserve_command(reservation(&id, 11, 12).unwrap())
            .await
            .unwrap()
        {
            ReservationOutcome::Replay(replay) => replay,
            other => panic!("completed same-input retry should replay, got {other:?}"),
        };
        assert_eq!(
            repo.lookup_command(&key, CommandLookupScope::CommandName("order.cancel"))
                .await
                .unwrap(),
            CommandLookup::Unknown,
            "status lookup must not disclose a command owned by a different route"
        );
        let lookup_replay = match repo
            .lookup_command(&key, CommandLookupScope::CommandName("order.create"))
            .await
            .unwrap()
        {
            CommandLookup::Replay(replay) => replay,
            other => panic!("completed command lookup should replay, got {other:?}"),
        };
        assert_eq!(reserved_replay, lookup_replay);
        assert_eq!(reserved_replay.state, CommandLedgerState::Accepted);
        assert_eq!(reserved_replay.causation_id, causation);
        assert_eq!(reserved_replay.outcome, expected_outcome);
        assert!(reserved_replay.projection_obligations.is_empty());
    }

    async fn concurrent_reservations_have_one_winner_and_one_causation<R>(repo: &R)
    where
        R: CommandLedgerAdapterConformance,
    {
        let id = Uuid::now_v7().to_string();
        let left = reservation(&id, 16, 17).unwrap();
        let right = reservation(&id, 16, 17).unwrap();
        let (left, right) = tokio::join!(repo.reserve_command(left), repo.reserve_command(right));
        let outcomes = (left.unwrap(), right.unwrap());
        let (winner, observed_causation) = match outcomes {
            (
                ReservationOutcome::Acquired(winner),
                ReservationOutcome::InProgress { causation_id },
            )
            | (
                ReservationOutcome::InProgress { causation_id },
                ReservationOutcome::Acquired(winner),
            ) => (winner, causation_id),
            other => panic!(
                "concurrent reservations should have one winner and one in-progress observer, got {other:?}"
            ),
        };
        assert_eq!(winner.causation_id(), &observed_causation);

        let completion = winner
            .complete(
                TerminalCommandState::Accepted,
                serde_json::json!({"concurrent_winner": true}),
                Duration::from_secs(300),
            )
            .unwrap();
        repo.commit_causal_batch(CausalCommitBatch::new(CommitBatch::empty(), completion))
            .await
            .unwrap();
    }

    async fn expired_lease_reclaims_through_the_adapter_clock<R>(repo: &R)
    where
        R: CommandLedgerAdapterConformance,
    {
        let id = Uuid::now_v7().to_string();
        let short_lease = reservation_for_partition_with_policy(
            &id,
            "v1:sha256:principal",
            "order.create",
            18,
            19,
            Duration::from_millis(100),
            Duration::from_secs(300),
        )
        .unwrap();
        let first = acquire(repo, short_lease).await;
        let causation = first.causation_id().clone();
        let first_token = first.attempt_token().as_str().to_string();

        tokio::time::sleep(Duration::from_millis(300)).await;
        let second = acquire(repo, reservation(&id, 18, 19).unwrap()).await;
        assert_eq!(second.causation_id(), &causation);
        assert_eq!(second.attempt_number(), 2);
        assert_ne!(second.attempt_token().as_str(), first_token);

        let completion = second
            .complete(
                TerminalCommandState::Accepted,
                serde_json::json!({"lease_reclaimed": true}),
                Duration::from_secs(300),
            )
            .unwrap();
        repo.commit_causal_batch(CausalCommitBatch::new(CommitBatch::empty(), completion))
            .await
            .unwrap();
    }

    async fn terminal_replays_are_deterministic<R>(repo: &R)
    where
        R: CommandLedgerAdapterConformance,
    {
        let terminal_states = [
            (TerminalCommandState::Accepted, CommandLedgerState::Accepted),
            (
                TerminalCommandState::AcceptedPendingProjection,
                CommandLedgerState::AcceptedPendingProjection,
            ),
            (
                TerminalCommandState::Projected,
                CommandLedgerState::Projected,
            ),
            (TerminalCommandState::Rejected, CommandLedgerState::Rejected),
        ];

        for (index, (terminal_state, ledger_state)) in terminal_states.into_iter().enumerate() {
            let id = Uuid::now_v7().to_string();
            let request = reservation(&id, 21, index as u8 + 1).unwrap();
            let key = request.key().clone();
            let attempt = acquire(repo, request).await;
            let causation = attempt.causation_id().clone();
            let expected_outcome = serde_json::json!({
                "terminal": ledger_state.as_str(),
                "index": index,
            });
            let expected_obligations =
                if terminal_state == TerminalCommandState::AcceptedPendingProjection {
                    vec![resolved_obligation(&format!("terminal-{index}"))]
                } else {
                    Vec::new()
                };
            let completion = attempt
                .complete_with_obligations(
                    terminal_state,
                    expected_outcome.clone(),
                    expected_obligations.clone(),
                    Duration::from_secs(300),
                )
                .unwrap();
            repo.commit_causal_batch(CausalCommitBatch::new(CommitBatch::empty(), completion))
                .await
                .unwrap();

            let first = match repo
                .lookup_command(&key, CommandLookupScope::CommandName("order.create"))
                .await
                .unwrap()
            {
                CommandLookup::Replay(replay) => replay,
                other => panic!("terminal lookup should replay, got {other:?}"),
            };
            let second = match repo
                .reserve_command(reservation(&id, 21, index as u8 + 1).unwrap())
                .await
                .unwrap()
            {
                ReservationOutcome::Replay(replay) => replay,
                other => panic!("terminal reservation should replay, got {other:?}"),
            };
            assert_eq!(first, second);
            assert_eq!(first.state, ledger_state);
            assert_eq!(first.causation_id, causation);
            assert_eq!(first.outcome, expected_outcome);
            assert_eq!(first.projection_obligations, expected_obligations);
        }
    }

    async fn response_loss_replays_outcome_and_projection_obligations<R>(repo: &R)
    where
        R: CommandLedgerAdapterConformance,
    {
        let id = Uuid::now_v7().to_string();
        let attempt = acquire(repo, reservation(&id, 31, 32).unwrap()).await;
        let causation = attempt.causation_id().clone();
        let expected_outcome = serde_json::json!({"order_id": "response-lost"});
        let expected_obligations = vec![resolved_obligation("response-loss")];
        let completion = attempt
            .complete_with_obligations(
                TerminalCommandState::AcceptedPendingProjection,
                expected_outcome.clone(),
                expected_obligations.clone(),
                Duration::from_secs(300),
            )
            .unwrap();

        // Model a committed transaction whose HTTP/GraphQL acknowledgement was
        // lost: the caller knows only that it must retry the same command ID.
        repo.commit_causal_batch(CausalCommitBatch::new(CommitBatch::empty(), completion))
            .await
            .unwrap();

        match repo
            .reserve_command(reservation(&id, 31, 32).unwrap())
            .await
            .unwrap()
        {
            ReservationOutcome::Replay(replay) => {
                assert_eq!(replay.state, CommandLedgerState::AcceptedPendingProjection);
                assert_eq!(replay.causation_id, causation);
                assert_eq!(replay.outcome, expected_outcome);
                assert_eq!(replay.projection_obligations, expected_obligations);
            }
            other => panic!("retry after response loss should replay, got {other:?}"),
        }
    }

    async fn retryable_unknown_reclaims_with_stable_causation<R>(repo: &R)
    where
        R: CommandLedgerAdapterConformance,
    {
        let id = Uuid::now_v7().to_string();
        let request = reservation(&id, 41, 42).unwrap();
        let key = request.key().clone();
        let first = acquire(repo, request).await;
        let causation = first.causation_id().clone();
        let first_token = first.attempt_token().as_str().to_string();
        repo.mark_retryable_unknown(first.fence()).await.unwrap();

        match repo
            .lookup_command(&key, CommandLookupScope::CommandName("order.create"))
            .await
            .unwrap()
        {
            CommandLookup::RetryableUnknown { causation_id } => {
                assert_eq!(causation_id, causation)
            }
            other => panic!("abandoned attempt should be retryable-unknown, got {other:?}"),
        }

        let second = acquire(repo, reservation(&id, 41, 42).unwrap()).await;
        assert_eq!(second.causation_id(), &causation);
        assert_eq!(second.attempt_number(), 2);
        assert_ne!(second.attempt_token().as_str(), first_token);
        let completion = second
            .complete(
                TerminalCommandState::Accepted,
                serde_json::json!({"reclaimed": true}),
                Duration::from_secs(300),
            )
            .unwrap();
        repo.commit_causal_batch(CausalCommitBatch::new(CommitBatch::empty(), completion))
            .await
            .unwrap();
    }

    async fn principal_partitions_are_isolated<R>(repo: &R)
    where
        R: CommandLedgerAdapterConformance,
    {
        let id = Uuid::now_v7().to_string();
        let first_request =
            reservation_for_partition(&id, "v1:sha256:principal-a", "order.create", 51, 52)
                .unwrap();
        let first_key = first_request.key().clone();
        let first = acquire(repo, first_request).await;
        let first_causation = first.causation_id().clone();
        let first_outcome = serde_json::json!({"partition": "a"});
        let completion = first
            .complete(
                TerminalCommandState::Accepted,
                first_outcome.clone(),
                Duration::from_secs(300),
            )
            .unwrap();
        repo.commit_causal_batch(CausalCommitBatch::new(CommitBatch::empty(), completion))
            .await
            .unwrap();

        let second_request =
            reservation_for_partition(&id, "v1:sha256:principal-b", "order.create", 51, 52)
                .unwrap();
        let second_key = second_request.key().clone();
        let second = acquire(repo, second_request).await;
        let second_causation = second.causation_id().clone();
        assert_ne!(second_causation, first_causation);
        let second_outcome = serde_json::json!({"partition": "b"});
        let completion = second
            .complete(
                TerminalCommandState::Rejected,
                second_outcome.clone(),
                Duration::from_secs(300),
            )
            .unwrap();
        repo.commit_causal_batch(CausalCommitBatch::new(CommitBatch::empty(), completion))
            .await
            .unwrap();

        let first_replay = match repo
            .lookup_command(&first_key, CommandLookupScope::CommandName("order.create"))
            .await
            .unwrap()
        {
            CommandLookup::Replay(replay) => replay,
            other => panic!("first principal should retain its replay, got {other:?}"),
        };
        let second_replay = match repo
            .lookup_command(&second_key, CommandLookupScope::CommandName("order.create"))
            .await
            .unwrap()
        {
            CommandLookup::Replay(replay) => replay,
            other => panic!("second principal should retain its replay, got {other:?}"),
        };
        assert_eq!(first_replay.state, CommandLedgerState::Accepted);
        assert_eq!(first_replay.outcome, first_outcome);
        assert_eq!(first_replay.causation_id, first_causation);
        assert_eq!(second_replay.state, CommandLedgerState::Rejected);
        assert_eq!(second_replay.outcome, second_outcome);
        assert_eq!(second_replay.causation_id, second_causation);
    }

    async fn committed_events_and_outbox_round_trip_ledger_causation<R>(repo: &R)
    where
        R: CommandLedgerAdapterConformance,
    {
        let id = Uuid::now_v7().to_string();
        let request = reservation(&id, 56, 57).unwrap();
        let key = request.key().clone();
        let attempt = acquire(repo, request).await;
        let causation = attempt.causation_id().as_str().to_string();

        let aggregate_id = format!("ledger-causation-stream-{}", Uuid::now_v7());
        let identity = StreamIdentity::new("command-ledger-conformance", &aggregate_id).unwrap();
        let mut entity = Entity::with_id(&aggregate_id);
        entity.set_causation_id("handler-event-causation-must-be-replaced");
        entity.digest_empty("CommandLedgerCausationEvent").unwrap();

        let outbox_id = format!("ledger-causation-outbox-{}", Uuid::now_v7());
        let mut message = OutboxMessage::create(
            outbox_id.clone(),
            "CommandLedgerCausationFact",
            b"{}".to_vec(),
        )
        .unwrap();
        message.set_causation_id("handler-outbox-causation-must-be-replaced");

        let completion = attempt
            .complete(
                TerminalCommandState::Accepted,
                serde_json::json!({"causation": "committed"}),
                Duration::from_secs(300),
            )
            .unwrap();
        let mut domain = CommitBatch::new(vec![StreamWrite::new(identity.clone(), &mut entity)]);
        domain.outbox_messages.push(message);
        repo.commit_causal_batch(CausalCommitBatch::new(domain, completion))
            .await
            .unwrap();

        let stored_stream = repo
            .get_stream(&identity)
            .await
            .unwrap()
            .expect("causal stream should persist");
        assert_eq!(stored_stream.events().len(), 1);
        assert_eq!(
            stored_stream.events()[0].causation_id(),
            Some(causation.as_str())
        );

        let stored_message = repo
            .outbox_store()
            .messages_by_status(OutboxMessageStatus::Pending, 1_000)
            .await
            .unwrap()
            .into_iter()
            .find(|message| message.id() == outbox_id)
            .expect("causal outbox message should persist");
        assert_eq!(stored_message.causation_id(), Some(causation.as_str()));
        match repo
            .lookup_command(&key, CommandLookupScope::CommandName("order.create"))
            .await
            .unwrap()
        {
            CommandLookup::Replay(replay) => {
                assert_eq!(replay.causation_id.as_str(), causation)
            }
            other => panic!("causal command should replay after commit, got {other:?}"),
        }
    }

    async fn stale_fence_rolls_back_every_commit_participant<R>(repo: &R)
    where
        R: CommandLedgerAdapterConformance,
    {
        let id = Uuid::now_v7().to_string();
        let key = reservation(&id, 61, 62).unwrap().key().clone();
        let first = acquire(repo, reservation(&id, 61, 62).unwrap()).await;
        repo.mark_retryable_unknown(first.fence()).await.unwrap();
        let second = acquire(repo, reservation(&id, 61, 62).unwrap()).await;
        let live_causation = second.causation_id().clone();

        let aggregate_id = format!("ledger-stale-stream-{}", Uuid::now_v7());
        let identity = StreamIdentity::new("command-ledger-conformance", &aggregate_id).unwrap();
        let mut entity = Entity::with_id(&aggregate_id);
        entity
            .digest_empty("CommandLedgerConformanceEvent")
            .unwrap();

        let outbox_id = format!("ledger-stale-outbox-{}", Uuid::now_v7());
        let outbox_message = OutboxMessage::create(
            outbox_id.clone(),
            "CommandLedgerConformanceFact",
            b"{}".to_vec(),
        )
        .unwrap();

        let read_model_id = format!("ledger-stale-view-{}", Uuid::now_v7());
        let view = LedgerConformanceView {
            id: read_model_id.clone(),
            marker: "must-roll-back".into(),
        };
        let mut read_models = ReadModelWritePlanBuilder::new();
        read_models.upsert(&view).unwrap();

        let inbox_consumer = format!("ledger-consumer-{}", Uuid::now_v7());
        let inbox_message_id = format!("ledger-message-{}", Uuid::now_v7());
        let snapshot = SnapshotRecord::new(
            identity.aggregate_type(),
            identity.aggregate_id(),
            1,
            1,
            vec![1, 2, 3],
        );

        let stale_completion = first
            .complete(
                TerminalCommandState::Accepted,
                serde_json::json!({"winner": false}),
                Duration::from_secs(300),
            )
            .unwrap();
        let mut domain = CommitBatch::new(vec![StreamWrite::new(identity.clone(), &mut entity)]);
        domain.outbox_messages.push(outbox_message);
        domain
            .read_model_plans
            .push(read_models.into_write_plan().unwrap());
        domain.snapshots.push(SnapshotWrite::Save {
            identity: identity.clone(),
            record: snapshot,
        });
        domain.inbox_receipts.push(InboxReceipt::new(
            inbox_consumer.clone(),
            inbox_message_id.clone(),
        ));

        let stale_result = repo
            .commit_causal_batch(CausalCommitBatch::new(domain, stale_completion))
            .await;
        assert!(
            matches!(stale_result, Err(CommandLedgerError::AttemptFenced { .. })),
            "stale causal commit should be fenced after every participant is staged, got {stale_result:?}"
        );

        assert!(repo.get_stream(&identity).await.unwrap().is_none());
        let pending = repo
            .outbox_store()
            .messages_by_status(OutboxMessageStatus::Pending, 1_000)
            .await
            .unwrap();
        assert!(pending.iter().all(|message| message.id() != outbox_id));
        let load = ReadModelWritePlanBuilder::new()
            .load::<LedgerConformanceView>(RowKey::new([("id", RowValue::String(read_model_id))]))
            .unwrap();
        assert!(repo.load_graph(load).await.unwrap().root.is_none());
        assert!(repo.get_snapshot(&identity).await.unwrap().is_none());
        assert!(!repo
            .inbox_contains(&inbox_consumer, &inbox_message_id)
            .await
            .unwrap());
        match repo
            .lookup_command(&key, CommandLookupScope::CommandName("order.create"))
            .await
            .unwrap()
        {
            CommandLookup::InProgress { causation_id } => {
                assert_eq!(causation_id, live_causation)
            }
            other => panic!("stale commit must leave live attempt untouched, got {other:?}"),
        }

        let live_completion = second
            .complete(
                TerminalCommandState::Accepted,
                serde_json::json!({"winner": true}),
                Duration::from_secs(300),
            )
            .unwrap();
        repo.commit_causal_batch(CausalCommitBatch::new(
            CommitBatch::empty(),
            live_completion,
        ))
        .await
        .unwrap();
    }

    async fn compacted_expiry_is_a_permanent_tombstone<R>(repo: &R)
    where
        R: CommandLedgerAdapterConformance,
    {
        let id = Uuid::now_v7().to_string();
        let request = reservation(&id, 71, 72).unwrap();
        let key = request.key().clone();
        let attempt = acquire(repo, request).await;
        let completion = attempt
            .complete(
                TerminalCommandState::Accepted,
                serde_json::json!({"short_lived": true}),
                Duration::from_millis(100),
            )
            .unwrap();
        repo.commit_causal_batch(CausalCommitBatch::new(CommitBatch::empty(), completion))
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(repo.compact_expired_commands(1_000).await.unwrap() >= 1);
        assert_eq!(
            repo.lookup_command(&key, CommandLookupScope::CommandName("order.create"))
                .await
                .unwrap(),
            CommandLookup::Expired
        );
        assert!(matches!(
            repo.reserve_command(reservation(&id, 71, 72).unwrap())
                .await
                .unwrap(),
            ReservationOutcome::Expired
        ));
        assert!(matches!(
            repo.reserve_command(reservation(&id, 99, 99).unwrap())
                .await
                .unwrap(),
            ReservationOutcome::Expired
        ));
    }

    async fn run_command_ledger_adapter_conformance<R>(repo: &R)
    where
        R: CommandLedgerAdapterConformance,
    {
        same_input_retries_and_identity_conflicts_conform(repo).await;
        concurrent_reservations_have_one_winner_and_one_causation(repo).await;
        expired_lease_reclaims_through_the_adapter_clock(repo).await;
        terminal_replays_are_deterministic(repo).await;
        response_loss_replays_outcome_and_projection_obligations(repo).await;
        retryable_unknown_reclaims_with_stable_causation(repo).await;
        principal_partitions_are_isolated(repo).await;
        committed_events_and_outbox_round_trip_ledger_causation(repo).await;
        stale_fence_rolls_back_every_commit_participant(repo).await;
        compacted_expiry_is_a_permanent_tombstone(repo).await;
    }

    #[test]
    fn command_id_requires_uuid_v7_and_canonicalizes() {
        let id = Uuid::now_v7().simple().to_string().to_uppercase();
        let parsed = CommandId::parse(id).unwrap();
        assert_eq!(
            Uuid::parse_str(parsed.as_str()).unwrap().get_version_num(),
            7
        );
        assert!(CommandId::parse("67e55044-10b1-426f-9247-bb680e5fe0c8").is_err());
        assert!(CommandId::parse("not-a-uuid").is_err());
    }

    #[test]
    fn uuid_v7_identities_require_the_rfc4122_variant() {
        let mut bytes = *Uuid::now_v7().as_bytes();
        bytes[8] &= 0x7f;
        let ncs_variant_v7 = Uuid::from_bytes(bytes);
        assert_eq!(ncs_variant_v7.get_version_num(), 7);
        assert_eq!(ncs_variant_v7.get_variant(), Variant::NCS);
        let spelling = ncs_variant_v7.hyphenated().to_string();

        assert!(matches!(
            CommandId::parse(&spelling),
            Err(CommandLedgerError::Invalid(_))
        ));
        assert!(matches!(
            CausationId::parse_stored(spelling.clone()),
            Err(CommandLedgerError::Corrupt(_))
        ));
        assert!(matches!(
            AttemptToken::parse_stored(spelling),
            Err(CommandLedgerError::Corrupt(_))
        ));

        let valid = Uuid::now_v7().to_string();
        assert!(CommandId::parse(&valid).is_ok());
        assert!(CausationId::parse_stored(valid.clone()).is_ok());
        assert!(AttemptToken::parse_stored(valid).is_ok());
    }

    #[test]
    fn prefixed_sha256_parser_is_checked_and_canonical() {
        let encoded = format!("sha256:{}", "ab".repeat(32));
        assert_eq!(
            CanonicalInputHash::parse_sha256(&encoded)
                .unwrap()
                .as_bytes(),
            &[0xab; 32]
        );
        assert!(CanonicalInputHash::parse_sha256(&"ab".repeat(32)).is_err());
        assert!(
            CommandContractFingerprint::parse_sha256(&format!("sha256:{}", "AB".repeat(32)))
                .is_err()
        );
        assert!(CommandContractFingerprint::parse_sha256("sha256:00").is_err());
    }

    #[test]
    fn contract_and_input_hashes_are_distinct_identity_components() {
        let id = Uuid::now_v7().to_string();
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
        let original = reservation(&id, 1, 2).unwrap();
        let row = CommandLedgerRecord::initial(&original, now).unwrap();

        let contract_drift = reservation(&id, 9, 2).unwrap();
        assert_eq!(
            row.classify_reservation(&contract_drift, now).unwrap(),
            ReservationDecision::Conflict
        );
        let input_drift = reservation(&id, 1, 9).unwrap();
        assert_eq!(
            row.classify_reservation(&input_drift, now).unwrap(),
            ReservationDecision::Conflict
        );
    }

    #[test]
    fn reclaim_preserves_causation_and_rotates_attempt_fence() {
        let id = Uuid::now_v7().to_string();
        let initial = reservation(&id, 1, 2).unwrap();
        let started = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
        let mut row = CommandLedgerRecord::initial(&initial, started).unwrap();
        let first_cause = row.causation_id.clone();
        let first_token = row.attempt_token.as_ref().unwrap().0.clone();

        let retry = reservation(&id, 1, 2).unwrap();
        let after_lease = started + Duration::from_secs(31);
        assert_eq!(
            row.classify_reservation(&retry, after_lease).unwrap(),
            ReservationDecision::Reclaim
        );
        row.reclaim(&retry, after_lease).unwrap();

        assert_eq!(row.causation_id, first_cause);
        assert_ne!(row.attempt_token.as_ref().unwrap().0, first_token);
        assert_eq!(row.attempt_number, 2);
    }

    #[test]
    fn stale_attempt_cannot_complete_after_reclaim() {
        let id = Uuid::now_v7().to_string();
        let initial = reservation(&id, 1, 2).unwrap();
        let started = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
        let mut row = CommandLedgerRecord::initial(&initial, started).unwrap();
        let stale = row.acquired_attempt().unwrap();

        let retry = reservation(&id, 1, 2).unwrap();
        row.reclaim(&retry, started + Duration::from_secs(31))
            .unwrap();
        let completion = stale
            .complete(
                TerminalCommandState::Accepted,
                serde_json::json!({"ok": true}),
                Duration::from_secs(300),
            )
            .unwrap();

        assert!(matches!(
            row.complete(&completion, started + Duration::from_secs(32)),
            Err(CommandLedgerError::AttemptFenced { .. })
        ));
    }

    #[test]
    fn completion_rejects_inconsistent_projection_obligation_states() {
        for state in [
            TerminalCommandState::Accepted,
            TerminalCommandState::Projected,
            TerminalCommandState::Rejected,
        ] {
            assert!(matches!(
                fresh_attempt().complete_with_obligations(
                    state,
                    serde_json::json!({"ok": true}),
                    vec![resolved_obligation("unexpected")],
                    Duration::from_secs(300),
                ),
                Err(CommandLedgerError::Invalid(_))
            ));
        }

        assert!(matches!(
            fresh_attempt().complete_with_obligations(
                TerminalCommandState::AcceptedPendingProjection,
                serde_json::json!({"ok": true}),
                Vec::new(),
                Duration::from_secs(300),
            ),
            Err(CommandLedgerError::Invalid(_))
        ));

        for state in [
            TerminalCommandState::Accepted,
            TerminalCommandState::Projected,
            TerminalCommandState::Rejected,
        ] {
            assert!(fresh_attempt()
                .complete(
                    state,
                    serde_json::json!({"ok": true}),
                    Duration::from_secs(300),
                )
                .is_ok());
        }
        assert!(fresh_attempt()
            .complete_with_obligations(
                TerminalCommandState::AcceptedPendingProjection,
                serde_json::json!({"ok": true}),
                vec![resolved_obligation("pending")],
                Duration::from_secs(300),
            )
            .is_ok());
    }

    #[test]
    fn completion_rejects_malformed_projection_obligations() {
        let mut blank_projector = resolved_obligation("blank-projector");
        blank_projector.projector = " \t".into();
        let mut blank_model = resolved_obligation("blank-model");
        blank_model.model = "\n".into();
        let mut empty_key = resolved_obligation("empty-key");
        empty_key.key.fields.clear();
        let mut blank_field = resolved_obligation("blank-field");
        blank_field.key.fields[0].field = "  ".into();
        let mut duplicate_field = resolved_obligation("duplicate-field");
        duplicate_field
            .key
            .fields
            .push(duplicate_field.key.fields[0].clone());

        for malformed in [
            blank_projector,
            blank_model,
            empty_key,
            blank_field,
            duplicate_field,
        ] {
            assert!(matches!(
                fresh_attempt().complete_with_obligations(
                    TerminalCommandState::AcceptedPendingProjection,
                    serde_json::json!({"ok": true}),
                    vec![malformed],
                    Duration::from_secs(300),
                ),
                Err(CommandLedgerError::Invalid(_))
            ));
        }
    }

    #[test]
    fn replay_rejects_inconsistent_projection_obligation_states() {
        for state in [
            CommandLedgerState::Accepted,
            CommandLedgerState::Projected,
            CommandLedgerState::Rejected,
        ] {
            let row = completed_replay_record(state, vec![resolved_obligation("unexpected")]);
            assert!(row.validate_stored_shape().is_ok());
            assert!(matches!(row.replay(), Err(CommandLedgerError::Corrupt(_))));
        }

        for state in [
            CommandLedgerState::AcceptedPendingProjection,
            CommandLedgerState::ProjectionFailed,
        ] {
            let row = completed_replay_record(state, Vec::new());
            assert!(row.validate_stored_shape().is_ok());
            assert!(matches!(row.replay(), Err(CommandLedgerError::Corrupt(_))));
        }

        let projection_failed = completed_replay_record(
            CommandLedgerState::ProjectionFailed,
            vec![resolved_obligation("failed")],
        );
        let replay = projection_failed.replay().unwrap();
        assert_eq!(replay.state, CommandLedgerState::ProjectionFailed);
        assert_eq!(replay.projection_obligations.len(), 1);
    }

    #[test]
    fn replay_rejects_malformed_projection_obligations() {
        let mut blank_projector = resolved_obligation("blank-projector");
        blank_projector.projector = " ".into();
        let mut blank_model = resolved_obligation("blank-model");
        blank_model.model.clear();
        let mut empty_key = resolved_obligation("empty-key");
        empty_key.key.fields.clear();
        let mut blank_field = resolved_obligation("blank-field");
        blank_field.key.fields[0].field = "\r\n".into();
        let mut duplicate_field = resolved_obligation("duplicate-field");
        duplicate_field
            .key
            .fields
            .push(duplicate_field.key.fields[0].clone());

        for malformed in [
            blank_projector,
            blank_model,
            empty_key,
            blank_field,
            duplicate_field,
        ] {
            let row = completed_replay_record(
                CommandLedgerState::AcceptedPendingProjection,
                vec![malformed],
            );
            assert!(row.validate_stored_shape().is_ok());
            assert!(matches!(row.replay(), Err(CommandLedgerError::Corrupt(_))));
        }
    }

    #[test]
    fn replay_validates_envelope_and_returns_only_the_outcome() {
        let id = Uuid::now_v7().to_string();
        let reservation = reservation(&id, 1, 2).unwrap();
        let started = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
        let mut row = CommandLedgerRecord::initial(&reservation, started).unwrap();
        let obligation = resolved_obligation("round-trip");
        let completion = row
            .acquired_attempt()
            .unwrap()
            .complete_with_obligations(
                TerminalCommandState::AcceptedPendingProjection,
                serde_json::json!({"order_id": "o-1"}),
                vec![obligation.clone()],
                Duration::from_secs(300),
            )
            .unwrap();
        row.complete(&completion, started + Duration::from_secs(1))
            .unwrap();
        let replay = row.replay().unwrap();
        assert_eq!(replay.outcome, serde_json::json!({"order_id": "o-1"}));
        assert_eq!(replay.projection_obligations, vec![obligation.clone()]);

        row.outcome_json = Some(r#"{"version":1,"outcome":null}"#.into());
        assert!(matches!(row.replay(), Err(CommandLedgerError::Corrupt(_))));

        let mut obligation_with_unknown_field = serde_json::to_value(obligation).unwrap();
        obligation_with_unknown_field
            .as_object_mut()
            .unwrap()
            .insert("unknown".into(), serde_json::json!(true));
        row.outcome_json = Some(
            serde_json::json!({
                "version": 1,
                "outcome": null,
                "projection_obligations": [obligation_with_unknown_field],
            })
            .to_string(),
        );
        assert!(matches!(row.replay(), Err(CommandLedgerError::Corrupt(_))));

        row.outcome_json =
            Some(r#"{"version":2,"outcome":null,"projection_obligations":[]}"#.into());
        assert!(matches!(row.replay(), Err(CommandLedgerError::Corrupt(_))));
    }

    #[test]
    fn causal_batch_applies_the_authoritative_stamp_at_the_final_boundary() {
        use crate::outbox::OutboxMessage;
        use crate::repository::StreamWrite;

        let id = Uuid::now_v7().to_string();
        let reservation = reservation(&id, 1, 2).unwrap();
        let row = CommandLedgerRecord::initial(&reservation, SystemTime::now()).unwrap();
        let attempt = row.acquired_attempt().unwrap();
        let causation = attempt.causation_id().as_str().to_string();
        let completion = attempt
            .complete(
                TerminalCommandState::Accepted,
                serde_json::json!({"ok": true}),
                Duration::from_secs(300),
            )
            .unwrap();

        let mut entity = Entity::with_id("order-1");
        entity.set_causation_id("handler-event-cause");
        entity.digest_empty("OrderCreated").unwrap();
        let mut message = OutboxMessage::create("fact-1", "OrderCreated", vec![]).unwrap();
        message.set_causation_id("handler-fact-cause");
        let mut domain = CommitBatch::new(vec![StreamWrite::new(
            StreamIdentity::new("order", "order-1").unwrap(),
            &mut entity,
        )]);
        domain.outbox_messages.push(message);

        let causal = CausalCommitBatch::new(domain, completion);
        assert_eq!(
            causal.domain.streams[0].entity.new_events()[0].causation_id(),
            Some(causation.as_str())
        );
        assert_eq!(
            causal.domain.outbox_messages[0].causation_id(),
            Some(causation.as_str())
        );
    }

    #[tokio::test]
    async fn in_memory_command_ledger_adapter_conformance() {
        use crate::in_memory_repo::InMemoryRepository;

        let repo = InMemoryRepository::new();
        repo.model_store()
            .register_schema::<LedgerConformanceView>()
            .unwrap();
        run_command_ledger_adapter_conformance(&repo).await;
    }

    #[cfg(feature = "sqlite")]
    #[tokio::test]
    async fn sqlite_command_ledger_adapter_conformance() {
        use crate::SqliteRepository;

        let repo = SqliteRepository::connect_and_migrate("sqlite::memory:")
            .await
            .unwrap();
        repo.bootstrap_table_schema_for_dev(&conformance_table_registry())
            .await
            .unwrap();
        run_command_ledger_adapter_conformance(&repo).await;
    }

    #[cfg(feature = "sqlite")]
    #[tokio::test]
    async fn sqlite_terminal_replay_survives_pool_drop_and_reopen() {
        use std::ffi::OsString;
        use std::path::PathBuf;

        use crate::SqliteRepository;

        struct TempSqliteDatabase {
            directory: PathBuf,
            database: PathBuf,
        }

        impl TempSqliteDatabase {
            fn new() -> Self {
                let directory = std::env::temp_dir().join(format!(
                    "distributed-command-ledger-restart-{}",
                    Uuid::now_v7()
                ));
                std::fs::create_dir(&directory).unwrap();
                let database = directory.join("ledger.sqlite3");
                Self {
                    directory,
                    database,
                }
            }

            fn url(&self) -> String {
                format!(
                    "sqlite://{}?mode=rwc",
                    self.database
                        .to_str()
                        .expect("temporary SQLite path must be valid UTF-8")
                )
            }
        }

        impl Drop for TempSqliteDatabase {
            fn drop(&mut self) {
                for suffix in ["", "-shm", "-wal", "-journal"] {
                    let mut path = OsString::from(self.database.as_os_str());
                    path.push(suffix);
                    let _ = std::fs::remove_file(PathBuf::from(path));
                }
                let _ = std::fs::remove_dir(&self.directory);
            }
        }

        let database = TempSqliteDatabase::new();
        let database_url = database.url();
        let repo = SqliteRepository::connect_and_migrate(&database_url)
            .await
            .unwrap();

        let command_id = Uuid::now_v7().to_string();
        let request = reservation(&command_id, 81, 82).unwrap();
        let key = request.key().clone();
        let attempt = acquire(&repo, request).await;
        let expected_causation = attempt.causation_id().clone();
        let expected_outcome = serde_json::json!({"order_id": "restart-order"});
        let expected_obligations = vec![resolved_obligation("sqlite-restart")];

        let aggregate_id = format!("sqlite-restart-stream-{}", Uuid::now_v7());
        let identity = StreamIdentity::new("command-ledger-restart", &aggregate_id).unwrap();
        let mut entity = Entity::with_id(&aggregate_id);
        entity.digest_empty("SqliteRestartCommitted").unwrap();
        let outbox_id = format!("sqlite-restart-outbox-{}", Uuid::now_v7());
        let outbox_message =
            OutboxMessage::create(&outbox_id, "SqliteRestartCommitted", b"{}".to_vec()).unwrap();

        let completion = attempt
            .complete_with_obligations(
                TerminalCommandState::AcceptedPendingProjection,
                expected_outcome.clone(),
                expected_obligations.clone(),
                Duration::from_secs(300),
            )
            .unwrap();
        let mut domain = CommitBatch::new(vec![StreamWrite::new(identity.clone(), &mut entity)]);
        domain.outbox_messages.push(outbox_message);
        repo.commit_causal_batch(CausalCommitBatch::new(domain, completion))
            .await
            .unwrap();

        repo.pool().close().await;
        drop(repo);

        let reopened = SqliteRepository::connect_and_migrate(&database_url)
            .await
            .unwrap();
        let replay = match reopened
            .lookup_command(&key, CommandLookupScope::CommandName("order.create"))
            .await
            .unwrap()
        {
            CommandLookup::Replay(replay) => replay,
            other => panic!("reopened SQLite ledger should replay, got {other:?}"),
        };
        assert_eq!(replay.state, CommandLedgerState::AcceptedPendingProjection);
        assert_eq!(replay.causation_id, expected_causation);
        assert_eq!(replay.outcome, expected_outcome);
        assert_eq!(replay.projection_obligations, expected_obligations);

        let stored_stream = reopened
            .get_stream(&identity)
            .await
            .unwrap()
            .expect("reopened SQLite repository should retain the causal event stream");
        assert_eq!(stored_stream.events().len(), 1);
        assert_eq!(
            stored_stream.events()[0].causation_id(),
            Some(expected_causation.as_str())
        );
        let stored_outbox = reopened
            .outbox_store()
            .messages_by_status(OutboxMessageStatus::Pending, 1_000)
            .await
            .unwrap()
            .into_iter()
            .find(|message| message.id() == outbox_id)
            .expect("reopened SQLite repository should retain the causal outbox fact");
        assert_eq!(
            stored_outbox.causation_id(),
            Some(expected_causation.as_str())
        );

        reopened.pool().close().await;
        drop(reopened);
    }

    #[cfg(feature = "postgres")]
    #[test]
    fn postgres_command_ledger_adapter_conformance_typechecks() {
        fn assert_conformance<R: CommandLedgerAdapterConformance>() {}
        assert_conformance::<crate::PostgresRepository>();
    }

    #[cfg(feature = "postgres")]
    #[tokio::test]
    async fn postgres_command_ledger_adapter_conformance_when_database_available() {
        use crate::PostgresRepository;

        let Ok(database_url) = std::env::var("DATABASE_URL") else {
            eprintln!("skipping Postgres command-ledger conformance test without DATABASE_URL");
            return;
        };
        let repo = PostgresRepository::connect_and_migrate(&database_url)
            .await
            .unwrap();
        repo.bootstrap_table_schema_for_dev(&conformance_table_registry())
            .await
            .unwrap();
        run_command_ledger_adapter_conformance(&repo).await;
    }

    #[tokio::test]
    async fn in_memory_adapter_reclaims_and_replays_with_a_stable_causation() {
        use crate::in_memory_repo::InMemoryRepository;

        let repo = InMemoryRepository::new();
        assert_eq!(
            repo.causal_storage_identity(),
            repo.clone().causal_storage_identity()
        );
        assert_ne!(
            repo.causal_storage_identity(),
            InMemoryRepository::new().causal_storage_identity()
        );

        let id = Uuid::now_v7().to_string();
        let first = match repo
            .reserve_command(reservation(&id, 1, 2).unwrap())
            .await
            .unwrap()
        {
            ReservationOutcome::Acquired(attempt) => attempt,
            other => panic!("expected acquired attempt, got {other:?}"),
        };
        let cause = first.causation_id().clone();
        let first_token = first.attempt_token().as_str().to_string();
        repo.mark_retryable_unknown(first.fence()).await.unwrap();

        let second = match repo
            .reserve_command(reservation(&id, 1, 2).unwrap())
            .await
            .unwrap()
        {
            ReservationOutcome::Acquired(attempt) => attempt,
            other => panic!("expected reclaimed attempt, got {other:?}"),
        };
        assert_eq!(second.causation_id(), &cause);
        assert_ne!(second.attempt_token().as_str(), first_token);
        assert_eq!(second.attempt_number(), 2);

        let completion = second
            .complete(
                TerminalCommandState::Accepted,
                serde_json::json!({"ok": true}),
                Duration::from_secs(300),
            )
            .unwrap();
        repo.commit_causal_batch(CausalCommitBatch::new(CommitBatch::empty(), completion))
            .await
            .unwrap();

        let key = reservation(&id, 1, 2).unwrap().key().clone();
        match repo
            .lookup_command(&key, CommandLookupScope::CommandName("order.create"))
            .await
            .unwrap()
        {
            CommandLookup::Replay(replay) => {
                assert_eq!(replay.causation_id, cause);
                assert_eq!(replay.outcome, serde_json::json!({"ok": true}));
            }
            other => panic!("expected replay, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn concurrent_in_memory_reservations_have_one_winner_and_one_cause() {
        use crate::in_memory_repo::InMemoryRepository;

        let repo = InMemoryRepository::new();
        let id = Uuid::now_v7().to_string();
        let left = reservation(&id, 5, 6).unwrap();
        let right = reservation(&id, 5, 6).unwrap();
        let (left, right) = tokio::join!(repo.reserve_command(left), repo.reserve_command(right));
        let outcomes = [left.unwrap(), right.unwrap()];
        let acquired = outcomes
            .iter()
            .filter(|outcome| matches!(outcome, ReservationOutcome::Acquired(_)))
            .count();
        assert_eq!(acquired, 1);
        let causes = outcomes
            .iter()
            .map(|outcome| match outcome {
                ReservationOutcome::Acquired(attempt) => attempt.causation_id().as_str(),
                ReservationOutcome::InProgress { causation_id } => causation_id.as_str(),
                other => panic!("unexpected concurrent reservation outcome: {other:?}"),
            })
            .collect::<Vec<_>>();
        assert_eq!(causes[0], causes[1]);
    }

    #[cfg(feature = "sqlite")]
    #[tokio::test]
    async fn sqlite_adapter_enforces_attempt_fence_and_replays() {
        use crate::outbox::{OutboxMessage, OutboxMessageStatus};
        use crate::outbox_worker::OutboxStore;
        use crate::SqliteRepository;

        let repo = SqliteRepository::connect_and_migrate("sqlite::memory:")
            .await
            .unwrap();
        let id = Uuid::now_v7().to_string();
        let first = match repo
            .reserve_command(reservation(&id, 3, 4).unwrap())
            .await
            .unwrap()
        {
            ReservationOutcome::Acquired(attempt) => attempt,
            other => panic!("expected acquired attempt, got {other:?}"),
        };
        let cause = first.causation_id().clone();
        repo.mark_retryable_unknown(first.fence()).await.unwrap();
        let second = match repo
            .reserve_command(reservation(&id, 3, 4).unwrap())
            .await
            .unwrap()
        {
            ReservationOutcome::Acquired(attempt) => attempt,
            other => panic!("expected reclaimed attempt, got {other:?}"),
        };
        assert_eq!(second.causation_id(), &cause);

        let stale = first
            .complete(
                TerminalCommandState::Accepted,
                serde_json::json!({"winner": false}),
                Duration::from_secs(300),
            )
            .unwrap();
        let mut stale_domain = CommitBatch::empty();
        stale_domain
            .outbox_messages
            .push(OutboxMessage::create("stale-effect", "ShouldRollback", vec![]).unwrap());
        assert!(matches!(
            repo.commit_causal_batch(CausalCommitBatch::new(stale_domain, stale))
                .await,
            Err(CommandLedgerError::AttemptFenced { .. })
        ));
        assert!(repo
            .outbox_store()
            .messages_by_status(OutboxMessageStatus::Pending, 10)
            .await
            .unwrap()
            .is_empty());

        let completion = second
            .complete(
                TerminalCommandState::Projected,
                serde_json::json!({"winner": true}),
                Duration::from_secs(300),
            )
            .unwrap();
        repo.commit_causal_batch(CausalCommitBatch::new(CommitBatch::empty(), completion))
            .await
            .unwrap();
        let key = reservation(&id, 3, 4).unwrap().key().clone();
        match repo
            .lookup_command(&key, CommandLookupScope::CommandName("order.create"))
            .await
            .unwrap()
        {
            CommandLookup::Replay(replay) => {
                assert_eq!(replay.state, CommandLedgerState::Projected);
                assert_eq!(replay.causation_id, cause);
                assert_eq!(replay.outcome, serde_json::json!({"winner": true}));
            }
            other => panic!("expected replay, got {other:?}"),
        }
    }

    #[test]
    fn expiry_is_a_permanent_compact_tombstone() {
        let id = Uuid::now_v7().to_string();
        let original = reservation(&id, 1, 2).unwrap();
        let started = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
        let mut row = CommandLedgerRecord::initial(&original, started).unwrap();
        row.expire(started + Duration::from_secs(301));

        let different = reservation(&id, 9, 9).unwrap();
        assert_eq!(
            row.classify_reservation(&different, started + Duration::from_secs(302))
                .unwrap(),
            ReservationDecision::Expire
        );
        assert!(row.outcome_json.is_none());
        assert!(row.attempt_token.is_none());
        assert_eq!(row.lookup().unwrap(), CommandLookup::Expired);
    }
}
