#[cfg(feature = "graphql")]
use std::time::Duration;

#[cfg(feature = "graphql")]
use serde_json::Value;

#[cfg(feature = "graphql")]
use crate::command_ledger::CausalTransactionalCommit;
#[cfg(feature = "graphql")]
use crate::command_ledger::{
    AttemptFence, CausalCommitBatch, CommandAttempt, CommandId, CommandLedgerError,
    CommandLedgerState, CommandLedgerStore, CommandLookup, CommandLookupScope, CommandReplay,
    TerminalCommandState,
};
#[cfg(feature = "graphql")]
use crate::graphql::command_contract::{CommandConsistency, TypedCommandContract};
#[cfg(feature = "graphql")]
use crate::microsvc::error::HandlerError;
#[cfg(feature = "graphql")]
use crate::microsvc::session::Session;
#[cfg(feature = "graphql")]
use crate::projection_protocol::{
    ProjectionCausationEvidenceRequest, ProjectionObligationEvidence,
    ProjectionObligationEvidenceBatchRequest, ProjectionObligationEvidenceRequest,
    ProjectionObservationKind, ProjectionProtocolStore, ProjectionRecordScope,
    SameTransactionProjectionEvidence,
};
#[cfg(feature = "graphql")]
use crate::repository::CommitBatch;

/// Stable transport classification for a typed causal command dispatch.
///
/// Public receipt/status envelopes map this private error set onto a stable
/// mutation edge without exposing repository details.
#[derive(Debug)]
#[cfg(feature = "graphql")]
pub(crate) enum CausalDispatchError {
    BadRequest(String),
    Forbidden,
    CommandIdReuse,
    InProgress,
    Expired,
    Rejected {
        code: &'static str,
        status: u16,
        message: String,
    },
    Handler(HandlerError),
    Internal(String),
}

#[cfg(feature = "graphql")]
impl CausalDispatchError {
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::BadRequest(_) => "BAD_REQUEST",
            Self::Forbidden => "FORBIDDEN",
            Self::CommandIdReuse => "COMMAND_ID_REUSE",
            Self::InProgress => "COMMAND_IN_PROGRESS",
            Self::Expired => "COMMAND_EXPIRED",
            Self::Rejected { code, .. } => code,
            Self::Handler(error) => match error.status_code() {
                400 => "BAD_REQUEST",
                401 => "UNAUTHORIZED",
                403 => "FORBIDDEN",
                404 => "NOT_FOUND",
                422 => "REJECTED",
                _ => "INTERNAL",
            },
            Self::Internal(_) => "INTERNAL",
        }
    }

    pub(crate) fn status_code(&self) -> u16 {
        match self {
            Self::BadRequest(_) => 400,
            Self::Forbidden => 403,
            Self::CommandIdReuse | Self::InProgress => 409,
            Self::Expired => 410,
            Self::Rejected { status, .. } => *status,
            Self::Handler(error) => error.status_code(),
            Self::Internal(_) => 500,
        }
    }

    pub(crate) fn client_message(&self) -> String {
        match self {
            Self::BadRequest(message) => message.clone(),
            Self::Rejected { message, .. } => message.clone(),
            Self::Forbidden => "command is not allowed".into(),
            Self::CommandIdReuse => "command ID was already used for different input".into(),
            Self::InProgress => "command is already in progress".into(),
            Self::Expired => "command ID has expired".into(),
            Self::Handler(error) => error.client_facing_message(),
            Self::Internal(_) => "internal error".into(),
        }
    }
}

#[cfg(feature = "graphql")]
impl std::fmt::Display for CausalDispatchError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Internal(detail) => formatter.write_str(detail),
            _ => formatter.write_str(&self.client_message()),
        }
    }
}

#[cfg(feature = "graphql")]
impl std::error::Error for CausalDispatchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Handler(error) => Some(error),
            _ => None,
        }
    }
}

#[cfg(feature = "graphql")]
impl From<HandlerError> for CausalDispatchError {
    fn from(error: HandlerError) -> Self {
        Self::Handler(error)
    }
}

/// Exact compiler-bound projection obligation retained by the durable command
/// replay.
///
/// The canonical scope remains a crate-private typed value. A transport layer
/// may hand it to the protocol token codec, but this type deliberately has no
/// serialization implementation that could expose topology, partition, or key
/// bytes directly.
#[cfg(feature = "graphql")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CausalCommandProjectionObligation {
    pub(crate) projector: String,
    pub(crate) model: String,
    pub(crate) scope: ProjectionRecordScope,
    pub(crate) observation_kind: ProjectionObservationKind,
}

/// Durable receipt material for one exact command attempt.
///
/// `direct_projection` is decoded from the versioned ledger replay envelope;
/// it is never reconstructed from the current read-model row.
#[cfg(feature = "graphql")]
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CausalCommandReceiptSource {
    pub(crate) command_id: String,
    pub(crate) causation_id: String,
    pub(crate) consistency: CommandConsistency,
    pub(crate) state: CommandLedgerState,
    pub(crate) outcome: Value,
    pub(crate) obligations: Vec<CausalCommandProjectionObligation>,
    pub(crate) projection_metadata: Option<crate::graphql::protocol::CommandProjectionMetadataV1>,
    pub(crate) direct_projection: Option<SameTransactionProjectionEvidence>,
}

#[cfg(feature = "graphql")]
impl CausalCommandReceiptSource {
    pub(super) fn from_replay(
        consistency: CommandConsistency,
        replay: CommandReplay,
    ) -> Result<Self, CausalDispatchError> {
        let direct_projection = replay
            .direct_projection
            .as_ref()
            .map(SameTransactionProjectionEvidence::from_replay_value)
            .transpose()
            .map_err(|error| {
                CausalDispatchError::Internal(format!(
                    "stored direct projection evidence is invalid: {error}"
                ))
            })?;
        let obligations = replay
            .projection_obligations
            .into_iter()
            .map(|obligation| CausalCommandProjectionObligation {
                projector: obligation.projector,
                model: obligation.model,
                scope: obligation.scope,
                // The current command compiler binds finite confirmations only
                // to relational records. Persisting dependency-vs-record kind
                // becomes mandatory before embedded confirmations are enabled.
                observation_kind: ProjectionObservationKind::Record,
            })
            .collect();
        let projection_metadata = replay
            .projection_metadata
            .as_deref()
            .map(crate::graphql::protocol::CommandProjectionMetadataV1::from_json)
            .transpose()
            .map_err(|error| {
                CausalDispatchError::Internal(format!(
                    "stored command projection metadata is invalid: {error}"
                ))
            })?;
        Ok(Self {
            command_id: replay.command_id.as_str().to_string(),
            causation_id: replay.causation_id.as_str().to_string(),
            consistency,
            state: replay.state,
            outcome: replay.outcome,
            obligations,
            projection_metadata,
            direct_projection,
        })
    }
}

/// Successful typed causal dispatch plus its exact durable receipt source.
#[cfg(feature = "graphql")]
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CausalDispatchResult {
    pub(crate) payload: Value,
    pub(crate) receipt: CausalCommandReceiptSource,
}

/// Stable public command-status vocabulary.
#[cfg(feature = "graphql")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CausalCommandPublicState {
    InProgress,
    Succeeded,
    SucceededPendingProjection,
    Projected,
    Rejected,
    ProjectionFailed,
    Expired,
    Unknown,
}

#[cfg(feature = "graphql")]
impl CausalCommandPublicState {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::InProgress => "in_progress",
            Self::Succeeded => "succeeded",
            Self::SucceededPendingProjection => "succeeded_pending_projection",
            Self::Projected => "projected",
            Self::Rejected => "rejected",
            Self::ProjectionFailed => "projection_failed",
            Self::Expired => "expired",
            Self::Unknown => "unknown",
        }
    }
}

/// Sanitized evidence state for one durable obligation.
///
/// A terminal failure is intentionally only a semantic marker. Failure IDs,
/// codes, bytes, digests, source cursors, and repair generations never cross
/// this service boundary.
#[cfg(feature = "graphql")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CausalProjectionEvidenceState {
    Pending,
    Observed,
    TerminalFailure,
}

#[cfg(feature = "graphql")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CausalCommandProjectionEvidence {
    pub(crate) obligation_index: usize,
    pub(crate) state: CausalProjectionEvidenceState,
    pub(crate) incarnation: Option<u64>,
    pub(crate) revision: Option<u64>,
}

/// Authorized, non-enumerating status for a client-created command ID.
///
/// Typed scopes and observations are crate-private inputs to the opaque token
/// codec. This type is not serializable and contains no raw failure material.
#[cfg(feature = "graphql")]
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CausalCommandPublicStatus {
    pub(crate) state: CausalCommandPublicState,
    pub(crate) command_id: String,
    pub(crate) causation_id: Option<String>,
    pub(crate) consistency: Option<CommandConsistency>,
    pub(crate) outcome: Option<Value>,
    pub(crate) obligations: Vec<CausalCommandProjectionObligation>,
    pub(crate) projection_metadata: Option<crate::graphql::protocol::CommandProjectionMetadataV1>,
    /// Historical modeled work was authenticated against an exact Draining
    /// binding, but its old-scope delta is not applicable to this client.
    pub(crate) projection_revalidate: bool,
    pub(crate) evidence: Vec<CausalCommandProjectionEvidence>,
    pub(crate) direct_projection: Option<SameTransactionProjectionEvidence>,
}

#[cfg(feature = "graphql")]
impl CausalCommandPublicStatus {
    pub(super) fn unknown(command_id: impl Into<String>) -> Self {
        Self {
            state: CausalCommandPublicState::Unknown,
            command_id: command_id.into(),
            causation_id: None,
            consistency: None,
            outcome: None,
            obligations: Vec::new(),
            projection_metadata: None,
            projection_revalidate: false,
            evidence: Vec::new(),
            direct_projection: None,
        }
    }

    pub(super) fn is_unknown(&self) -> bool {
        self.state == CausalCommandPublicState::Unknown
    }
}

/// Error returned when attaching a GraphQL engine whose typed command
/// inventory is not exactly the executable service inventory, or whose query
/// storage cannot prove the identity required by a `Projected` command.
#[cfg(feature = "graphql")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphqlServiceBindError(pub String);

#[cfg(feature = "graphql")]
impl std::fmt::Display for GraphqlServiceBindError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[cfg(feature = "graphql")]
impl std::error::Error for GraphqlServiceBindError {}

#[cfg(feature = "graphql")]
pub(super) fn ensure_causal_grant(
    contract: &TypedCommandContract,
    session: &Session,
) -> Result<(), CausalDispatchError> {
    if contract.roles.is_empty()
        || session
            .role()
            .is_some_and(|role| contract.roles.iter().any(|allowed| allowed == role))
    {
        Ok(())
    } else {
        Err(CausalDispatchError::Forbidden)
    }
}

#[cfg(feature = "graphql")]
pub(super) fn causal_handler_error_code(error: &HandlerError) -> &'static str {
    match error.status_code() {
        400 => "BAD_REQUEST",
        401 => "UNAUTHORIZED",
        403 => "FORBIDDEN",
        404 => "NOT_FOUND",
        422 => "REJECTED",
        _ => "REJECTED",
    }
}

#[cfg(feature = "graphql")]
pub(super) fn internal_ledger_error(error: CommandLedgerError) -> CausalDispatchError {
    CausalDispatchError::Internal(error.to_string())
}

#[cfg(feature = "graphql")]
pub(super) fn replay_result(
    consistency: CommandConsistency,
    replay: CommandReplay,
) -> Result<CausalDispatchResult, CausalDispatchError> {
    match replay.state {
        CommandLedgerState::Succeeded
        | CommandLedgerState::SucceededPendingProjection
        | CommandLedgerState::Projected
        | CommandLedgerState::ProjectionFailed => {
            let receipt = CausalCommandReceiptSource::from_replay(consistency, replay)?;
            Ok(CausalDispatchResult {
                payload: receipt.outcome.clone(),
                receipt,
            })
        }
        CommandLedgerState::Rejected => replay_rejection(replay.outcome),
        CommandLedgerState::InProgress
        | CommandLedgerState::RetryableUnknown
        | CommandLedgerState::Expired => Err(CausalDispatchError::Internal(
            "stored replay has a non-terminal state".into(),
        )),
    }
}

#[cfg(feature = "graphql")]
pub(super) fn replay_rejection(
    outcome: Value,
) -> Result<CausalDispatchResult, CausalDispatchError> {
    let error = outcome
        .get("error")
        .and_then(Value::as_object)
        .ok_or_else(|| CausalDispatchError::Internal("stored rejection is malformed".into()))?;
    let code = match error.get("code").and_then(Value::as_str) {
        Some("BAD_REQUEST") => "BAD_REQUEST",
        Some("UNAUTHORIZED") => "UNAUTHORIZED",
        Some("FORBIDDEN") => "FORBIDDEN",
        Some("NOT_FOUND") => "NOT_FOUND",
        Some("REJECTED") => "REJECTED",
        _ => {
            return Err(CausalDispatchError::Internal(
                "stored rejection code is invalid".into(),
            ));
        }
    };
    let status = error
        .get("status")
        .and_then(Value::as_u64)
        .and_then(|status| u16::try_from(status).ok())
        .filter(|status| (400..500).contains(status))
        .ok_or_else(|| {
            CausalDispatchError::Internal("stored rejection status is invalid".into())
        })?;
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .ok_or_else(|| CausalDispatchError::Internal("stored rejection message is invalid".into()))?
        .to_string();
    Err(CausalDispatchError::Rejected {
        code,
        status,
        message,
    })
}

#[cfg(feature = "graphql")]
pub(super) async fn commit_causal_rejection<R>(
    repository: &R,
    attempt: CommandAttempt,
    consistency: CommandConsistency,
    retention: Duration,
    code: &'static str,
    status: u16,
    message: String,
) -> Result<CausalDispatchResult, CausalDispatchError>
where
    R: CommandLedgerStore + CausalTransactionalCommit + Send + Sync,
{
    let outcome = serde_json::json!({
        "error": {
            "code": code,
            "status": status,
            "message": message,
        }
    });
    let fence = attempt.fence();
    let completion = attempt
        .complete(TerminalCommandState::Rejected, outcome, retention)
        .map_err(internal_ledger_error)?;
    match repository
        .commit_causal_batch(CausalCommitBatch::new(CommitBatch::empty(), completion))
        .await
    {
        Ok(()) => Err(CausalDispatchError::Rejected {
            code,
            status,
            message,
        }),
        Err(error) => {
            recover_causal_commit_error(repository, fence, consistency, error.to_string()).await
        }
    }
}

#[cfg(feature = "graphql")]
pub(super) async fn load_committed_dispatch_result<R>(
    repository: &R,
    fence: &AttemptFence,
    consistency: CommandConsistency,
) -> Result<CausalDispatchResult, CausalDispatchError>
where
    R: CommandLedgerStore + Send + Sync,
{
    match repository
        .lookup_command(fence.key(), CommandLookupScope::Attempt(fence))
        .await
        .map_err(internal_ledger_error)?
    {
        CommandLookup::Replay(replay) => replay_result(consistency, replay),
        CommandLookup::Expired => Err(CausalDispatchError::Expired),
        CommandLookup::InProgress { .. }
        | CommandLookup::RetryableUnknown { .. }
        | CommandLookup::Unknown => Err(CausalDispatchError::Internal(
            "committed command has no exact durable replay receipt".into(),
        )),
    }
}

#[cfg(feature = "graphql")]
pub(super) async fn abandon_causal_attempt<R>(
    repository: &R,
    attempt: CommandAttempt,
    consistency: CommandConsistency,
    detail: String,
) -> Result<CausalDispatchResult, CausalDispatchError>
where
    R: CommandLedgerStore + Send + Sync,
{
    let fence = attempt.fence();
    match repository.mark_retryable_unknown(fence.clone()).await {
        Ok(()) => Err(CausalDispatchError::Internal(detail)),
        Err(CommandLedgerError::AttemptFenced { .. }) => {
            resolve_ambiguous_lookup(repository, fence, consistency, detail).await
        }
        Err(error) => Err(CausalDispatchError::Internal(format!(
            "{detail}; failed to mark command retryable: {error}"
        ))),
    }
}

#[cfg(feature = "graphql")]
pub(super) async fn recover_causal_commit_error<R>(
    repository: &R,
    fence: AttemptFence,
    consistency: CommandConsistency,
    detail: String,
) -> Result<CausalDispatchResult, CausalDispatchError>
where
    R: CommandLedgerStore + Send + Sync,
{
    resolve_ambiguous_lookup(repository, fence, consistency, detail).await
}

#[cfg(feature = "graphql")]
pub(super) async fn resolve_ambiguous_lookup<R>(
    repository: &R,
    fence: AttemptFence,
    consistency: CommandConsistency,
    detail: String,
) -> Result<CausalDispatchResult, CausalDispatchError>
where
    R: CommandLedgerStore + Send + Sync,
{
    match repository
        .lookup_command(fence.key(), CommandLookupScope::Attempt(&fence))
        .await
    {
        Ok(CommandLookup::Replay(replay)) => replay_result(consistency, replay),
        Ok(CommandLookup::Expired) => Err(CausalDispatchError::Expired),
        Ok(CommandLookup::RetryableUnknown { .. }) => Err(CausalDispatchError::Internal(detail)),
        Ok(CommandLookup::InProgress { .. }) => {
            match repository.mark_retryable_unknown(fence).await {
                Ok(()) => Err(CausalDispatchError::Internal(detail)),
                Err(CommandLedgerError::AttemptFenced { .. }) => {
                    Err(CausalDispatchError::InProgress)
                }
                Err(error) => Err(CausalDispatchError::Internal(format!(
                    "{detail}; command recovery failed: {error}"
                ))),
            }
        }
        Ok(CommandLookup::Unknown) => Err(CausalDispatchError::Internal(format!(
            "{detail}; command ledger row disappeared"
        ))),
        Err(error) => Err(CausalDispatchError::Internal(format!(
            "{detail}; command outcome lookup failed: {error}"
        ))),
    }
}

#[cfg(feature = "graphql")]
pub(super) async fn evaluate_causal_command_status<R>(
    repository: &R,
    command_id: &CommandId,
    consistency: CommandConsistency,
    lookup: CommandLookup,
    protocol: Option<&crate::graphql::protocol::ProtocolResponseAccumulator>,
) -> Result<CausalCommandPublicStatus, CausalDispatchError>
where
    R: CommandLedgerStore + ProjectionProtocolStore + Send + Sync,
{
    match lookup {
        CommandLookup::Unknown => Ok(CausalCommandPublicStatus::unknown(command_id.as_str())),
        CommandLookup::Expired => Ok(CausalCommandPublicStatus {
            state: CausalCommandPublicState::Expired,
            command_id: command_id.as_str().to_string(),
            causation_id: None,
            consistency: Some(consistency),
            outcome: None,
            obligations: Vec::new(),
            projection_metadata: None,
            projection_revalidate: false,
            evidence: Vec::new(),
            direct_projection: None,
        }),
        CommandLookup::InProgress { causation_id }
        | CommandLookup::RetryableUnknown { causation_id } => Ok(CausalCommandPublicStatus {
            state: CausalCommandPublicState::InProgress,
            command_id: command_id.as_str().to_string(),
            causation_id: Some(causation_id.as_str().to_string()),
            consistency: Some(consistency),
            outcome: None,
            obligations: Vec::new(),
            projection_metadata: None,
            projection_revalidate: false,
            evidence: Vec::new(),
            direct_projection: None,
        }),
        CommandLookup::Replay(replay) => {
            let receipt = CausalCommandReceiptSource::from_replay(consistency, replay)?;
            let modeled_plan = match receipt.projection_metadata.as_ref() {
                Some(metadata) => Some(
                    protocol
                        .ok_or_else(|| {
                            CausalDispatchError::Internal(
                                "modeled projection status requires authenticated protocol authority"
                                    .into(),
                            )
                        })?
                        .modeled_projection_evidence_topologies(
                            &receipt.causation_id,
                            metadata,
                        )
                        .map_err(|error| {
                            CausalDispatchError::Internal(format!(
                                "modeled projection status authority rejected its stored identity: {error}"
                            ))
                        })?,
                ),
                None => None,
            };
            let projection_revalidate = modeled_plan.as_ref().is_some_and(|plan| {
                plan.disposition
                    == crate::graphql::projection_delta::runtime::ModeledProjectionStatusDisposition::Revalidate
            });
            let (state, evidence) = match receipt.state {
                CommandLedgerState::Succeeded => (CausalCommandPublicState::Succeeded, Vec::new()),
                CommandLedgerState::Projected => (
                    CausalCommandPublicState::Projected,
                    (0..receipt
                        .projection_metadata
                        .as_ref()
                        .map_or(receipt.obligations.len(), |metadata| {
                            metadata.obligations.len()
                        }))
                        .map(|obligation_index| CausalCommandProjectionEvidence {
                            obligation_index,
                            state: CausalProjectionEvidenceState::Observed,
                            // The durable ledger state proves every finite
                            // obligation. Exact record positions are optional
                            // status detail and are not reconstructed from a
                            // later row head.
                            incarnation: None,
                            revision: None,
                        })
                        .collect(),
                ),
                CommandLedgerState::Rejected => (CausalCommandPublicState::Rejected, Vec::new()),
                CommandLedgerState::ProjectionFailed => {
                    (CausalCommandPublicState::ProjectionFailed, Vec::new())
                }
                CommandLedgerState::SucceededPendingProjection => {
                    evaluate_pending_projection_evidence(
                        repository,
                        &receipt,
                        protocol,
                        modeled_plan.as_ref(),
                    )
                    .await?
                }
                CommandLedgerState::InProgress
                | CommandLedgerState::RetryableUnknown
                | CommandLedgerState::Expired => {
                    return Err(CausalDispatchError::Internal(format!(
                        "stored replay has non-terminal state `{}`",
                        receipt.state.as_str()
                    )));
                }
            };
            Ok(CausalCommandPublicStatus {
                state,
                command_id: receipt.command_id,
                causation_id: Some(receipt.causation_id),
                consistency: Some(receipt.consistency),
                outcome: Some(receipt.outcome),
                obligations: receipt.obligations,
                projection_metadata: receipt.projection_metadata,
                projection_revalidate,
                evidence,
                direct_projection: receipt.direct_projection,
            })
        }
    }
}

#[cfg(feature = "graphql")]
pub(super) async fn evaluate_pending_projection_evidence<R>(
    repository: &R,
    receipt: &CausalCommandReceiptSource,
    protocol: Option<&crate::graphql::protocol::ProtocolResponseAccumulator>,
    modeled_plan: Option<&crate::graphql::projection_delta::runtime::ModeledProjectionStatusPlan>,
) -> Result<
    (
        CausalCommandPublicState,
        Vec<CausalCommandProjectionEvidence>,
    ),
    CausalDispatchError,
>
where
    R: ProjectionProtocolStore + Send + Sync,
{
    if let Some(metadata) = receipt.projection_metadata.as_ref() {
        return evaluate_pending_modeled_projection_evidence(
            repository,
            receipt,
            metadata,
            protocol.ok_or_else(|| {
                CausalDispatchError::Internal(
                    "modeled projection status requires authenticated protocol authority".into(),
                )
            })?,
            modeled_plan.ok_or_else(|| {
                CausalDispatchError::Internal(
                    "modeled projection status is missing its authenticated status plan".into(),
                )
            })?,
        )
        .await;
    }

    let requests = receipt
        .obligations
        .iter()
        .map(|obligation| {
            ProjectionObligationEvidenceRequest::new(
                receipt.causation_id.clone(),
                obligation.scope.clone(),
                obligation.observation_kind,
            )
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            CausalDispatchError::Internal(format!(
                "stored projection obligation cannot be evaluated: {error}"
            ))
        })?;
    let request = ProjectionObligationEvidenceBatchRequest::new(requests).map_err(|error| {
        CausalDispatchError::Internal(format!(
            "stored projection obligation batch is invalid: {error}"
        ))
    })?;
    let batch = repository
        .projection_obligation_evidence_batch(&request)
        .await
        .map_err(|error| {
            CausalDispatchError::Internal(format!(
                "projection obligation evidence lookup failed: {error}"
            ))
        })?;
    if batch.evidence.len() != receipt.obligations.len() {
        return Err(CausalDispatchError::Internal(format!(
            "projection obligation evidence returned {} items for {} exact probes",
            batch.evidence.len(),
            receipt.obligations.len()
        )));
    }

    let mut evidence = Vec::with_capacity(batch.evidence.len());
    for (obligation_index, (obligation, item)) in
        receipt.obligations.iter().zip(batch.evidence).enumerate()
    {
        let item = match item {
            ProjectionObligationEvidence::Pending => CausalCommandProjectionEvidence {
                obligation_index,
                state: CausalProjectionEvidenceState::Pending,
                incarnation: None,
                revision: None,
            },
            ProjectionObligationEvidence::TerminalFailure(_) => CausalCommandProjectionEvidence {
                obligation_index,
                state: CausalProjectionEvidenceState::TerminalFailure,
                incarnation: None,
                revision: None,
            },
            ProjectionObligationEvidence::Observed(observation) => {
                if observation.causation_id != receipt.causation_id
                    || observation.kind != obligation.observation_kind
                    || observation.scope != obligation.scope
                {
                    return Err(CausalDispatchError::Internal(
                        "projection store returned evidence outside the exact obligation probe"
                            .into(),
                    ));
                }
                let (incarnation, revision) = match observation.revision.as_ref() {
                    Some(record)
                        if obligation.observation_kind == ProjectionObservationKind::Record
                            && record.scope() == &obligation.scope =>
                    {
                        (Some(record.incarnation()), Some(record.revision()))
                    }
                    None if obligation.observation_kind
                        == ProjectionObservationKind::Dependency =>
                    {
                        (None, None)
                    }
                    _ => {
                        return Err(CausalDispatchError::Internal(
                            "projection store returned an invalid revision for the obligation kind"
                                .into(),
                        ));
                    }
                };
                CausalCommandProjectionEvidence {
                    obligation_index,
                    state: CausalProjectionEvidenceState::Observed,
                    incarnation,
                    revision,
                }
            }
        };
        evidence.push(item);
    }

    // Failure precedence is intentional: a terminal failure must never be
    // hidden by observations from the remaining obligations.
    let state = collapse_projection_evidence(&evidence);
    Ok((state, evidence))
}

#[cfg(feature = "graphql")]
async fn evaluate_pending_modeled_projection_evidence<R>(
    repository: &R,
    receipt: &CausalCommandReceiptSource,
    metadata: &crate::graphql::protocol::CommandProjectionMetadataV1,
    protocol: &crate::graphql::protocol::ProtocolResponseAccumulator,
    plan: &crate::graphql::projection_delta::runtime::ModeledProjectionStatusPlan,
) -> Result<
    (
        CausalCommandPublicState,
        Vec<CausalCommandProjectionEvidence>,
    ),
    CausalDispatchError,
>
where
    R: ProjectionProtocolStore + Send + Sync,
{
    let request = ProjectionCausationEvidenceRequest::new(
        receipt.causation_id.clone(),
        plan.topologies.clone(),
    )
    .map_err(|error| {
        CausalDispatchError::Internal(format!(
            "stored modeled projection causation is invalid: {error}"
        ))
    })?;
    let batch = repository
        .projection_causation_evidence(&request)
        .await
        .map_err(|error| {
            CausalDispatchError::Internal(format!(
                "modeled projection causation evidence lookup failed: {error}"
            ))
        })?;
    let modeled = protocol
        .modeled_projection_evidence(&receipt.causation_id, metadata, &batch, plan.disposition)
        .map_err(|error| {
            CausalDispatchError::Internal(format!(
                "modeled projection evidence authority rejected durable candidates: {error}"
            ))
        })?;
    if modeled.len() != metadata.obligations.len() {
        return Err(CausalDispatchError::Internal(format!(
            "modeled projection evidence returned {} items for {} opaque obligations",
            modeled.len(),
            metadata.obligations.len()
        )));
    }
    let evidence = modeled
        .into_iter()
        .enumerate()
        .map(|(obligation_index, item)| Ok(match item {
            crate::graphql::projection_delta::runtime::ModeledProjectionEvidence::Pending => {
                CausalCommandProjectionEvidence {
                    obligation_index,
                    state: CausalProjectionEvidenceState::Pending,
                    incarnation: None,
                    revision: None,
                }
            }
            crate::graphql::projection_delta::runtime::ModeledProjectionEvidence::TerminalFailure => {
                CausalCommandProjectionEvidence {
                    obligation_index,
                    state: CausalProjectionEvidenceState::TerminalFailure,
                    incarnation: None,
                    revision: None,
                }
            }
            crate::graphql::projection_delta::runtime::ModeledProjectionEvidence::Observed(
                observation,
            ) => {
                let (incarnation, revision) = match observation.revision.as_ref() {
                    Some(record)
                        if observation.kind == ProjectionObservationKind::Record
                            && record.scope() == &observation.scope =>
                    {
                        (Some(record.incarnation()), Some(record.revision()))
                    }
                    None if observation.kind == ProjectionObservationKind::Dependency => {
                        (None, None)
                    }
                    _ => {
                        return Err(CausalDispatchError::Internal(
                            "modeled projection store returned invalid observation revision"
                                .into(),
                        ));
                    }
                };
                CausalCommandProjectionEvidence {
                    obligation_index,
                    state: CausalProjectionEvidenceState::Observed,
                    incarnation,
                    revision,
                }
            }
        }))
        .collect::<Result<Vec<_>, _>>()?;
    let state = collapse_projection_evidence(&evidence);
    Ok((state, evidence))
}

#[cfg(feature = "graphql")]
pub(super) fn collapse_projection_evidence(
    evidence: &[CausalCommandProjectionEvidence],
) -> CausalCommandPublicState {
    if evidence
        .iter()
        .any(|item| item.state == CausalProjectionEvidenceState::TerminalFailure)
    {
        CausalCommandPublicState::ProjectionFailed
    } else if !evidence.is_empty()
        && evidence
            .iter()
            .all(|item| item.state == CausalProjectionEvidenceState::Observed)
    {
        CausalCommandPublicState::Projected
    } else {
        CausalCommandPublicState::SucceededPendingProjection
    }
}
