use std::any::TypeId;
use std::marker::PhantomData;

use serde::{Deserialize, Serialize};

use super::direct_projection::ResolvedDirectProjectionTarget;
use super::projection_proof::{CommandCommitProofError, ProjectionCommitProof};
use super::typed_command::TypedCommandContract;
use crate::graphql::types::GraphqlOutputType;
use crate::outbox::OutboxMessage;
use crate::projection_protocol::SameTransactionProjectionBatch;
use crate::read_model::RelationalReadModel;
use crate::table::{TableSchema, TableWritePlan};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandConsistency {
    /// The command was accepted. With no confirmation plan this is terminal;
    /// with an explicit finite plan it is accepted pending projection.
    Accepted,
    /// A durable fact was committed and declared projectors are expected.
    Fact,
    /// The returned view was committed in the command transaction.
    Projected,
}

mod sealed {
    pub trait Outcome {}
    pub trait PreparableOutcome {}
}

/// A committed accepted command result.
///
/// There is intentionally no public constructor. The durable command
/// committer is the only framework component allowed to create this wrapper.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Accepted<T> {
    payload: T,
}

/// A committed durable-fact command result.
///
/// There is intentionally no public constructor. The durable command
/// committer is the only framework component allowed to create this wrapper.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fact<T> {
    payload: T,
}

/// A committed same-transaction projection result.
///
/// There is intentionally no public constructor. The durable command
/// committer is the only framework component allowed to create this wrapper.
/// `T` must be a relational read model, and preparation is available only
/// through the framework-owned workspace that stages the exact row upsert.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Projected<T> {
    payload: T,
}

macro_rules! committed_outcome {
    ($wrapper:ident, $kind:expr) => {
        impl<T> sealed::Outcome for $wrapper<T> {}

        impl<T> CommandOutcome for $wrapper<T>
        where
            T: GraphqlOutputType + Serialize + Send + Sync + 'static,
        {
            type Payload = T;
            const CONSISTENCY: CommandConsistency = $kind;

            fn payload(&self) -> &T {
                &self.payload
            }

            fn __finalize_committed(payload: T) -> Self {
                Self::from_committed_payload(payload)
            }
        }
    };
}

committed_outcome!(Accepted, CommandConsistency::Accepted);
committed_outcome!(Fact, CommandConsistency::Fact);

impl<T> sealed::Outcome for Projected<T> where T: RelationalReadModel {}

impl<T> CommandOutcome for Projected<T>
where
    T: GraphqlOutputType + RelationalReadModel + Serialize + Send + Sync + 'static,
{
    type Payload = T;
    const CONSISTENCY: CommandConsistency = CommandConsistency::Projected;

    fn payload(&self) -> &T {
        &self.payload
    }

    fn __finalize_committed(payload: T) -> Self {
        Self::from_committed_payload(payload)
    }

    fn __projected_model() -> Option<(TypeId, &'static TableSchema)> {
        Some((TypeId::of::<T>(), T::schema()))
    }
}

macro_rules! crate_committed_constructor {
    ($wrapper:ident) => {
        impl<T> $wrapper<T> {
            /// The ledger-aware committer is the only intended caller.
            pub(crate) fn from_committed_payload(payload: T) -> Self {
                Self { payload }
            }
        }
    };
}

crate_committed_constructor!(Accepted);
crate_committed_constructor!(Fact);

impl<T> Projected<T>
where
    T: RelationalReadModel,
{
    /// Created only after a proof-bearing staged projection commits.
    fn from_committed_payload(payload: T) -> Self {
        Self { payload }
    }
}

impl<T> sealed::PreparableOutcome for Accepted<T> {}
impl<T> sealed::PreparableOutcome for Fact<T> {}

/// Sealed type-level contract implemented by committed command outcomes.
pub trait CommandOutcome: sealed::Outcome + Send + Sync + 'static {
    type Payload: GraphqlOutputType + Serialize + Send + Sync + 'static;
    const CONSISTENCY: CommandConsistency;

    fn payload(&self) -> &Self::Payload;

    #[doc(hidden)]
    fn __finalize_committed(payload: Self::Payload) -> Self;

    /// Compiler-only model identity retained by an ordinary
    /// `typed_command::<I, Projected<M>>` declaration. The sealed default keeps
    /// accepted/fact outcomes unbound while `Projected<M>` supplies its exact
    /// relational schema without an application-facing projection target API.
    #[doc(hidden)]
    fn __projected_model() -> Option<(TypeId, &'static TableSchema)> {
        None
    }
}

/// Error produced while serializing a completion before commit I/O.
#[derive(Debug)]
pub enum PrepareCommandError {
    Serialize(serde_json::Error),
}

impl std::fmt::Display for PrepareCommandError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Serialize(error) => {
                write!(formatter, "command payload serialization failed: {error}")
            }
        }
    }
}

impl std::error::Error for PrepareCommandError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Serialize(error) => Some(error),
        }
    }
}

impl From<serde_json::Error> for PrepareCommandError {
    fn from(error: serde_json::Error) -> Self {
        Self::Serialize(error)
    }
}

/// A serialized command completion waiting for the atomic command committer.
///
/// Preparing is deliberately separate from returning a committed outcome: it
/// proves serialization before transaction I/O while keeping both the durable
/// outcome and the declaration-owned confirmation plan outside application
/// handler control.
pub struct PreparedCommand<K: CommandOutcome> {
    payload: K::Payload,
    serialized_payload: serde_json::Value,
    projection_proof: Option<ProjectionCommitProof>,
    _outcome: PhantomData<fn() -> K>,
}

impl<K: CommandOutcome> PreparedCommand<K> {
    fn prepare_payload(payload: K::Payload) -> Result<Self, PrepareCommandError> {
        let serialized_payload = serde_json::to_value(&payload)?;
        Ok(Self {
            payload,
            serialized_payload,
            projection_proof: None,
            _outcome: PhantomData,
        })
    }

    pub fn consistency(&self) -> CommandConsistency {
        K::CONSISTENCY
    }

    pub fn serialized_payload(&self) -> &serde_json::Value {
        &self.serialized_payload
    }

    /// Validate declaration-owned completion obligations against exactly what
    /// the handler staged. This runs before any commit I/O.
    pub(crate) fn validate_commit_evidence(
        &self,
        contract: &TypedCommandContract,
        has_staged_aggregate_events: bool,
        outbox_messages: &[OutboxMessage],
        read_model_plans: &[TableWritePlan],
    ) -> Result<(), CommandCommitProofError> {
        if contract.consistency != K::CONSISTENCY {
            return Err(CommandCommitProofError::ConsistencyMismatch {
                declared: contract.consistency,
                prepared: K::CONSISTENCY,
            });
        }
        if contract.output_type_id != TypeId::of::<K::Payload>() {
            return Err(CommandCommitProofError::OutputTypeMismatch);
        }

        match K::CONSISTENCY {
            CommandConsistency::Accepted | CommandConsistency::Fact => {
                if self.projection_proof.is_some() {
                    return Err(CommandCommitProofError::UnexpectedProjectionProof);
                }
                contract.validate_outbox_fact_coverage(outbox_messages)
            }
            CommandConsistency::Projected => {
                if !has_staged_aggregate_events && outbox_messages.is_empty() {
                    return Err(CommandCommitProofError::DurableFactMissing);
                }
                if !contract.confirmations.is_empty() {
                    return Err(CommandCommitProofError::ProjectedHasConfirmations);
                }
                self.projection_proof
                    .as_ref()
                    .ok_or(CommandCommitProofError::MissingProjectionProof)?
                    .validate(contract.output_type_id, read_model_plans)
            }
        }
    }

    /// The durable committer is the sole intended consumer.
    pub(crate) fn finalize_after_commit(self) -> (K, serde_json::Value) {
        (
            K::__finalize_committed(self.payload),
            self.serialized_payload,
        )
    }

    /// Remove the proof-matched projected upsert from ordinary table plans and
    /// seal it as the repository's causal direct-projection participant.
    ///
    /// Accepted and Fact commands return no participant. A Projected command
    /// must have exactly one resolved declaration-owned target; the extracted
    /// mutation is never also submitted through the legacy/raw plan path.
    pub(crate) fn seal_direct_projection(
        &self,
        target: Option<ResolvedDirectProjectionTarget>,
        read_model_plans: &mut Vec<TableWritePlan>,
        causation_id: &str,
    ) -> Result<Option<SameTransactionProjectionBatch>, CommandCommitProofError> {
        match K::CONSISTENCY {
            CommandConsistency::Accepted | CommandConsistency::Fact => {
                if target.is_some() {
                    return Err(CommandCommitProofError::UnexpectedDirectProjectionTarget);
                }
                Ok(None)
            }
            CommandConsistency::Projected => {
                let target =
                    target.ok_or(CommandCommitProofError::MissingDirectProjectionTarget)?;
                let proof = self
                    .projection_proof
                    .as_ref()
                    .ok_or(CommandCommitProofError::MissingProjectionProof)?;
                let mutation =
                    proof.extract_exact_upsert(TypeId::of::<K::Payload>(), read_model_plans)?;
                target
                    .seal(mutation, causation_id)
                    .map(Some)
                    .map_err(|error| CommandCommitProofError::DirectProjection(error.to_string()))
            }
        }
    }
}

impl<M> PreparedCommand<Projected<M>>
where
    M: GraphqlOutputType + RelationalReadModel + Serialize + Send + Sync + 'static,
{
    /// Build a projected completion from the exact model value whose full-row
    /// upsert was staged by the framework-owned causal workspace.
    pub(crate) fn prepare_projected(
        payload: M,
        proof: ProjectionCommitProof,
    ) -> Result<Self, PrepareCommandError> {
        let serialized_payload = serde_json::to_value(&payload)?;
        Ok(Self {
            payload,
            serialized_payload,
            projection_proof: Some(proof),
            _outcome: PhantomData,
        })
    }
}

impl<K> PreparedCommand<K>
where
    K: CommandOutcome + sealed::PreparableOutcome,
{
    /// Prepare an accepted or fact payload for the durable committer.
    /// Projected results require a staged transactional proof and do
    /// not implement the private preparation capability.
    pub fn prepare(payload: K::Payload) -> Result<Self, PrepareCommandError> {
        Self::prepare_payload(payload)
    }
}
