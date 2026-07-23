//! Typed command consistency, prepared completions, and portable client effects.
//!
//! This module deliberately separates declaration from durable completion. A
//! handler may prepare a typed payload, but it cannot choose which projection
//! confirmations count. That finite plan belongs to the command declaration,
//! and only the framework-owned command-ledger committer may turn a preparation into
//! an [`Accepted`], [`Fact`], or [`Projected`] value.

#![cfg_attr(not(feature = "graphql"), allow(dead_code))]

use std::any::TypeId;
use std::collections::{BTreeMap, BTreeSet};
use std::marker::PhantomData;
use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde::ser::{
    SerializeMap, SerializeSeq, SerializeStruct, SerializeStructVariant, SerializeTuple,
    SerializeTupleStruct, SerializeTupleVariant,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::types::{GraphqlInputType, GraphqlOutputType, GraphqlTypeDef};
use crate::outbox::OutboxMessage;
use crate::projection_protocol::{
    ProjectionEpoch, ProjectionModelOwnership, ProjectionPartition, ProjectionPartitionSpec,
    ProjectionProtocolError, ProjectionScopeCodec, ProjectorTopologyId, ResolvedProjectionKey,
    ResolvedProjectionKeyField, ResolvedProjectionObligation, SameTransactionProjectionBatch,
};
use crate::read_model::RelationalReadModel;
use crate::table::{
    RowKey, RowValue, RowValues, RowWriteMode, TableMutation, TableSchema, TableStoreError,
    TableWritePlan,
};

/// The consistency guarantee declared by a typed command handler.
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

/// Why staged command work cannot prove its declared durable outcome.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CommandCommitProofError {
    ConsistencyMismatch {
        declared: CommandConsistency,
        prepared: CommandConsistency,
    },
    OutputTypeMismatch,
    DurableFactMissing,
    FactHasNoConfirmations,
    UnreachableConfirmation {
        projector: String,
        expected_facts: Vec<String>,
        staged_facts: Vec<String>,
    },
    UnexpectedProjectionProof,
    MissingProjectionProof,
    ProjectedHasConfirmations,
    ProjectionOutputTypeMismatch,
    ProjectionWriteMissing {
        model: String,
    },
    ProjectionWriteConflict {
        model: String,
    },
    ProjectionWriteMismatch {
        model: String,
    },
    MissingDirectProjectionTarget,
    UnexpectedDirectProjectionTarget,
    DirectProjection(String),
}

impl std::fmt::Display for CommandCommitProofError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ConsistencyMismatch { declared, prepared } => write!(
                formatter,
                "prepared command consistency {prepared:?} does not match declaration {declared:?}"
            ),
            Self::OutputTypeMismatch => {
                formatter.write_str("prepared command output type does not match its declaration")
            }
            Self::DurableFactMissing => formatter.write_str(
                "fact and projected commands require a staged aggregate event or outbox fact",
            ),
            Self::FactHasNoConfirmations => formatter.write_str(
                "a fact command requires at least one finite projector confirmation",
            ),
            Self::UnreachableConfirmation {
                projector,
                expected_facts,
                staged_facts,
            } => write!(
                formatter,
                "projector `{projector}` cannot be reached: expected one of {expected_facts:?}, staged outbox facts were {staged_facts:?}"
            ),
            Self::UnexpectedProjectionProof => formatter.write_str(
                "accepted and fact commands cannot carry a same-transaction projection proof",
            ),
            Self::MissingProjectionProof => formatter.write_str(
                "projected command did not stage its returned read model as an exact upsert",
            ),
            Self::ProjectedHasConfirmations => formatter.write_str(
                "projected command cannot declare asynchronous projector confirmations",
            ),
            Self::ProjectionOutputTypeMismatch => formatter.write_str(
                "projected command proof is for a different Rust read-model type",
            ),
            Self::ProjectionWriteMissing { model } => write!(
                formatter,
                "projected command did not stage an upsert for returned model `{model}`"
            ),
            Self::ProjectionWriteConflict { model } => write!(
                formatter,
                "projected command staged more than one mutation for returned model `{model}` and key"
            ),
            Self::ProjectionWriteMismatch { model } => write!(
                formatter,
                "projected command staged a row that differs from returned model `{model}`"
            ),
            Self::MissingDirectProjectionTarget => formatter.write_str(
                "projected command has no declaration-owned direct projection target",
            ),
            Self::UnexpectedDirectProjectionTarget => formatter.write_str(
                "accepted and fact commands cannot carry a direct projection target",
            ),
            Self::DirectProjection(error) => {
                write!(formatter, "direct projection target could not be sealed: {error}")
            }
        }
    }
}

impl std::error::Error for CommandCommitProofError {}

/// Private evidence tying a `Projected<M>` payload to one exact full-row
/// upsert. Application handlers can obtain this only through the causal
/// workspace's stage-and-prepare operation.
pub(crate) struct ProjectionCommitProof {
    model_type_id: TypeId,
    model_name: String,
    table_name: String,
    key_fingerprint: String,
    row_fingerprint: String,
}

impl ProjectionCommitProof {
    pub(crate) fn for_model<M>(model: &M) -> Result<Self, TableStoreError>
    where
        M: RelationalReadModel + 'static,
    {
        let schema = M::schema();
        schema.validate()?;
        let key = model.primary_key()?;
        let row = model.to_row()?;
        Ok(Self {
            model_type_id: TypeId::of::<M>(),
            model_name: schema.model_name.clone(),
            table_name: schema.table_name.clone(),
            key_fingerprint: fingerprint_key(&key),
            row_fingerprint: fingerprint_row(&row),
        })
    }

    fn validate(
        &self,
        output_type_id: TypeId,
        plans: &[TableWritePlan],
    ) -> Result<(), CommandCommitProofError> {
        if self.model_type_id != output_type_id {
            return Err(CommandCommitProofError::ProjectionOutputTypeMismatch);
        }

        let mut target_count = 0usize;
        let mut exact_match = false;
        for mutation in plans.iter().flat_map(|plan| &plan.mutations) {
            let (schema, key) = match mutation {
                TableMutation::UpsertRow(mutation) => (mutation.schema, &mutation.key),
                TableMutation::PatchRow(mutation) => (mutation.schema, &mutation.key),
                TableMutation::DeleteRow(mutation) => (mutation.schema, &mutation.key),
            };
            if schema.table_name != self.table_name || fingerprint_key(key) != self.key_fingerprint
            {
                continue;
            }

            target_count += 1;
            if let TableMutation::UpsertRow(mutation) = mutation {
                exact_match = mutation.mode == RowWriteMode::Upsert
                    && mutation.schema.model_name == self.model_name
                    && fingerprint_row(&mutation.values) == self.row_fingerprint;
            }
        }

        match target_count {
            0 => Err(CommandCommitProofError::ProjectionWriteMissing {
                model: self.model_name.clone(),
            }),
            1 if exact_match => Ok(()),
            1 => Err(CommandCommitProofError::ProjectionWriteMismatch {
                model: self.model_name.clone(),
            }),
            _ => Err(CommandCommitProofError::ProjectionWriteConflict {
                model: self.model_name.clone(),
            }),
        }
    }

    fn extract_exact_upsert(
        &self,
        output_type_id: TypeId,
        plans: &mut Vec<TableWritePlan>,
    ) -> Result<TableMutation, CommandCommitProofError> {
        self.validate(output_type_id, plans)?;

        let mut found = None;
        for (plan_index, plan) in plans.iter().enumerate() {
            for (mutation_index, mutation) in plan.mutations.iter().enumerate() {
                let TableMutation::UpsertRow(row) = mutation else {
                    continue;
                };
                if row.schema.table_name == self.table_name
                    && row.schema.model_name == self.model_name
                    && fingerprint_key(&row.key) == self.key_fingerprint
                    && row.mode == RowWriteMode::Upsert
                    && fingerprint_row(&row.values) == self.row_fingerprint
                {
                    found = Some((plan_index, mutation_index));
                }
            }
        }
        let (plan_index, mutation_index) =
            found.ok_or_else(|| CommandCommitProofError::ProjectionWriteMissing {
                model: self.model_name.clone(),
            })?;
        let mutation = plans[plan_index].mutations.remove(mutation_index);
        if plans[plan_index].mutations.is_empty() {
            plans.remove(plan_index);
        } else {
            plans[plan_index].validate().map_err(|_| {
                CommandCommitProofError::ProjectionWriteConflict {
                    model: self.model_name.clone(),
                }
            })?;
        }
        Ok(mutation)
    }
}

fn fingerprint_key(key: &RowKey) -> String {
    fingerprint_values("distributed.command-projection-key.v1", key.iter())
}

fn fingerprint_row(row: &RowValues) -> String {
    fingerprint_values("distributed.command-projection-row.v1", row.iter())
}

fn fingerprint_values<'a>(
    domain: &str,
    values: impl Iterator<Item = (&'a str, &'a RowValue)>,
) -> String {
    let canonical = values
        .map(|(column, value)| serde_json::json!([column, canonical_row_value(value)]))
        .collect::<Vec<_>>();
    let mut digest = Sha256::new();
    digest.update(domain.as_bytes());
    digest.update([0]);
    digest.update(
        serde_json::to_vec(&canonical)
            .expect("canonical row projection fingerprint serialization cannot fail"),
    );
    format!("sha256:{:x}", digest.finalize())
}

fn canonical_row_value(value: &RowValue) -> serde_json::Value {
    match value {
        RowValue::Null => serde_json::json!(["null"]),
        RowValue::Bool(value) => serde_json::json!(["bool", value]),
        RowValue::I64(value) => serde_json::json!(["i64", value.to_string()]),
        RowValue::U64(value) => serde_json::json!(["u64", value.to_string()]),
        RowValue::F64(value) => serde_json::json!(["f64_bits", value.to_bits().to_string()]),
        RowValue::String(value) => serde_json::json!(["string", value]),
        RowValue::Bytes(value) => serde_json::json!(["bytes", value]),
        RowValue::Json(value) => serde_json::json!(["json", canonical_json(value)]),
    }
}

fn canonical_json(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.iter().map(canonical_json).collect())
        }
        serde_json::Value::Object(values) => {
            let mut entries = values.iter().collect::<Vec<_>>();
            entries.sort_by(|left, right| left.0.cmp(right.0));
            serde_json::Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key.clone(), canonical_json(value)))
                    .collect(),
            )
        }
        scalar => scalar.clone(),
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

/// Generator evaluated exactly once into the canonical command input before
/// hashing, optimistic overlay evaluation, or dispatch (runtime task 9).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum InputDefaultGenerator {
    UuidV7,
    Ulid,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CommandInputDefault {
    pub path: Vec<String>,
    pub generator: InputDefaultGenerator,
}

/// Portable value expression in a command effect.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum EffectExpression {
    Input {
        path: Vec<String>,
    },
    TrustedPreset {
        name: String,
    },
    Constant {
        value: serde_json::Value,
    },
    /// SQL null, emitted only by the type-checked `null()` expression.
    Null,
    /// Construction-time serialization failure. This private sentinel is
    /// rejected before contract fingerprinting or Surface/manifest emission.
    #[serde(skip)]
    InvalidConstant {
        error: String,
    },
}

/// One model-field assignment in portable effect IR.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct EffectFieldValue {
    pub field: String,
    pub value: EffectExpression,
}

/// A complete, ordered model key in portable effect IR.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct EffectKey {
    pub fields: Vec<EffectFieldValue>,
}

/// One declaration-owned projector/model/key confirmation target.
///
/// The dispatcher resolves these expressions from the retained canonical
/// GraphQL wire input before commit I/O, then commits the finite obligations
/// atomically with the command ledger/fact. Handlers cannot add, remove, or
/// rewrite targets.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct CommandProjectionConfirmation {
    pub projector: String,
    pub model: String,
    pub key: EffectKey,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub partition: Option<EffectExpression>,
    /// Frozen declaration identity used for server-side topology validation
    /// and typed service binding. It is intentionally absent from role client
    /// manifests, whose projector catalog already carries authorized topology.
    #[serde(skip_serializing)]
    projector_topology: ProjectorTopologyIdentity,
    /// Exact server-side topology identity compiled from accepted facts, the
    /// versioned scope codec, and every complete owned table schema. Typed
    /// declarations start unbound; Surface/engine compilation must attach this
    /// before an obligation can be lowered or committed.
    #[serde(skip_serializing)]
    protocol_topology: Option<ProjectorTopologyId>,
    #[serde(skip_serializing)]
    schema: Option<&'static TableSchema>,
}

/// Why a declaration-owned projection obligation could not be resolved before
/// commit I/O.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ProjectionObligationResolutionError {
    MissingInputPath {
        projector: String,
        model: String,
        target: String,
        path: Vec<String>,
    },
    TrustedPresetUnavailable {
        projector: String,
        model: String,
        target: String,
        preset: String,
    },
    InvalidConstant {
        projector: String,
        model: String,
        target: String,
        error: String,
    },
    InvalidBinding {
        projector: String,
        model: String,
        reason: String,
    },
}

impl std::fmt::Display for ProjectionObligationResolutionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingInputPath {
                projector,
                model,
                target,
                path,
            } => write!(
                formatter,
                "projection obligation `{projector}`/`{model}` {target} references absent canonical input path `{}`",
                path.join("."),
            ),
            Self::TrustedPresetUnavailable {
                projector,
                model,
                target,
                preset,
            } => write!(
                formatter,
                "projection obligation `{projector}`/`{model}` {target} uses unavailable trusted preset `{preset}`",
            ),
            Self::InvalidConstant {
                projector,
                model,
                target,
                error,
            } => write!(
                formatter,
                "projection obligation `{projector}`/`{model}` {target} contains an invalid constant: {error}",
            ),
            Self::InvalidBinding {
                projector,
                model,
                reason,
            } => write!(
                formatter,
                "projection obligation `{projector}`/`{model}` is not bound to an exact topology: {reason}",
            ),
        }
    }
}

impl std::error::Error for ProjectionObligationResolutionError {}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ProjectorTopologyIdentity {
    name: String,
    facts: Vec<String>,
    models: Vec<String>,
    partition: ProjectionPartitionSpec,
}

impl ProjectorTopologyIdentity {
    fn new(
        name: &str,
        facts: &[String],
        models: &[String],
        partition: &ProjectionPartitionSpec,
    ) -> Self {
        let mut facts = facts.to_vec();
        facts.sort();
        facts.dedup();
        let mut models = models.to_vec();
        models.sort();
        models.dedup();
        Self {
            name: name.to_string(),
            facts,
            models,
            partition: partition.clone(),
        }
    }

    fn canonical_value(&self) -> serde_json::Value {
        serde_json::json!({
            "name": self.name,
            "facts": self.facts,
            "models": self.models,
            "partition": self.partition,
        })
    }
}

impl CommandProjectionConfirmation {
    pub(crate) fn canonical_value(&self) -> serde_json::Value {
        serde_json::json!({
            "projector": self.projector,
            "projector_topology": self.projector_topology.canonical_value(),
            "protocol_topology": self.protocol_topology.as_ref().map(|topology| serde_json::json!({
                "version": topology.version(),
                "name": topology.name(),
                "digest": topology.digest(),
            })),
            "model": self.model,
            "key": self.key,
            "partition": self.partition,
        })
    }

    pub(crate) fn topology_matches(
        &self,
        name: &str,
        facts: &[String],
        models: &[String],
        partition: &ProjectionPartitionSpec,
    ) -> bool {
        self.projector_topology == ProjectorTopologyIdentity::new(name, facts, models, partition)
    }

    pub(crate) fn bind_protocol_topology(&mut self, topology: ProjectorTopologyId) {
        self.protocol_topology = Some(topology);
    }

    pub(crate) fn protocol_topology(&self) -> Option<&ProjectorTopologyId> {
        self.protocol_topology.as_ref()
    }

    pub(crate) fn clear_protocol_topology(&mut self) {
        self.protocol_topology = None;
    }

    pub(crate) fn partition_matches(&self, partition: &ProjectionPartitionSpec) -> bool {
        match partition {
            ProjectionPartitionSpec::Unit => self.partition.is_none(),
            ProjectionPartitionSpec::Constant { value } => {
                self.partition
                    == Some(EffectExpression::Constant {
                        value: value.clone(),
                    })
            }
            ProjectionPartitionSpec::InputPath { .. } => self.partition.is_some(),
        }
    }
}

pub(crate) fn validate_projection_confirmation_count(
    command_name: &str,
    count: usize,
) -> Result<(), String> {
    if count > crate::projection_protocol::MAX_PROJECTION_EVIDENCE_BATCH_ITEMS {
        return Err(format!(
            "typed command `{command_name}` declares {count} projector confirmations; maximum is {}",
            crate::projection_protocol::MAX_PROJECTION_EVIDENCE_BATCH_ITEMS
        ));
    }
    Ok(())
}

/// Compiler-retained relational identity for one ordinary `Projected<M>`
/// declaration before the GraphQL Surface resolves its unique physical owner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CommandProjectedModel {
    pub(crate) output_type_id: TypeId,
    pub(crate) model: String,
    pub(crate) table: String,
    pub(crate) schema: &'static TableSchema,
    pub(crate) partition: Option<EffectExpression>,
}

impl CommandProjectedModel {
    fn new(output_type_id: TypeId, schema: &'static TableSchema) -> Self {
        Self {
            output_type_id,
            model: schema.model_name.clone(),
            table: schema.table_name.clone(),
            schema,
            partition: None,
        }
    }

    fn canonical_value(&self) -> serde_json::Value {
        serde_json::json!({
            "model": self.model,
            "table": self.table,
            "partition": self.partition,
        })
    }

    pub(crate) fn partition_matches(&self, partition: &ProjectionPartitionSpec) -> bool {
        match partition {
            ProjectionPartitionSpec::Unit => self.partition.is_none(),
            ProjectionPartitionSpec::Constant { value } => {
                self.partition
                    == Some(EffectExpression::Constant {
                        value: value.clone(),
                    })
            }
            ProjectionPartitionSpec::InputPath { .. } => self.partition.is_some(),
        }
    }

    pub(crate) fn bind(
        &self,
        projector: &str,
        facts: &[String],
        models: &[String],
        projector_partition: &ProjectionPartitionSpec,
        change_epoch: Option<&str>,
        mut ownership: Vec<ProjectionModelOwnership>,
        protocol_topology: Option<ProjectorTopologyId>,
    ) -> CommandDirectProjectionTarget {
        ownership.sort_by(|left, right| {
            (left.model.as_str(), left.table.as_str())
                .cmp(&(right.model.as_str(), right.table.as_str()))
        });
        CommandDirectProjectionTarget {
            projector: projector.to_string(),
            model: self.model.clone(),
            table: self.table.clone(),
            output_type_id: self.output_type_id,
            projector_topology: ProjectorTopologyIdentity::new(
                projector,
                facts,
                models,
                projector_partition,
            ),
            protocol_topology,
            partition: self.partition.clone(),
            change_epoch: change_epoch.map(str::to_string),
            schema: self.schema,
            ownership,
        }
    }
}

/// Compiler-owned direct target for one `Projected<M>` command.
///
/// This metadata is deliberately hidden from ordinary handler code. Generated
/// declarations bind it once; application handlers still only call
/// `context.projected(view)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CommandDirectProjectionTarget {
    pub(crate) projector: String,
    pub(crate) model: String,
    pub(crate) table: String,
    pub(crate) output_type_id: TypeId,
    projector_topology: ProjectorTopologyIdentity,
    /// Exact post-bind protocol identity compiled from accepted facts, the
    /// versioned scope codec, and every complete owned table schema. The
    /// pre-bind typed declaration deliberately carries `None` and cannot
    /// resolve into a direct projection participant.
    protocol_topology: Option<ProjectorTopologyId>,
    pub(crate) partition: Option<EffectExpression>,
    pub(crate) change_epoch: Option<String>,
    pub(crate) schema: &'static TableSchema,
    /// Complete frozen model → physical-table inventory owned by the
    /// projector topology. Bootstrap claims this entire set atomically even
    /// though the direct command mutates exactly one output model.
    pub(crate) ownership: Vec<ProjectionModelOwnership>,
}

impl CommandDirectProjectionTarget {
    pub(crate) fn canonical_value(&self) -> serde_json::Value {
        serde_json::json!({
            "projector": self.projector,
            "projector_topology": self.projector_topology.canonical_value(),
            "protocol_topology": self.protocol_topology.as_ref().map(|topology| serde_json::json!({
                "version": topology.version(),
                "name": topology.name(),
                "digest": topology.digest(),
            })),
            "model": self.model,
            "table": self.table,
            "partition": self.partition,
            "change_epoch": self.change_epoch,
            "ownership": self.ownership.iter().map(|owner| serde_json::json!({
                "model": owner.model,
                "table": owner.table,
            })).collect::<Vec<_>>(),
        })
    }

    pub(crate) fn topology_matches(
        &self,
        name: &str,
        facts: &[String],
        models: &[String],
        projector_partition: &ProjectionPartitionSpec,
        change_epoch: Option<&str>,
    ) -> bool {
        self.projector_topology
            == ProjectorTopologyIdentity::new(name, facts, models, projector_partition)
            && self.change_epoch.as_deref() == change_epoch
    }

    pub(crate) fn protocol_topology_matches(&self, topology: &ProjectorTopologyId) -> bool {
        self.protocol_topology.as_ref() == Some(topology)
    }

    pub(crate) fn partition_matches(&self, partition: &ProjectionPartitionSpec) -> bool {
        match partition {
            ProjectionPartitionSpec::Unit => self.partition.is_none(),
            ProjectionPartitionSpec::Constant { value } => {
                self.partition
                    == Some(EffectExpression::Constant {
                        value: value.clone(),
                    })
            }
            ProjectionPartitionSpec::InputPath { .. } => self.partition.is_some(),
        }
    }

    pub(crate) fn resolve(
        &self,
        canonical_wire_input: &serde_json::Value,
    ) -> Result<ResolvedDirectProjectionTarget, DirectProjectionTargetResolutionError> {
        let change_epoch = self.change_epoch.as_ref().ok_or_else(|| {
            DirectProjectionTargetResolutionError::InvalidTarget {
                projector: self.projector.clone(),
                model: self.model.clone(),
                reason: "registered projector has no change-log epoch".into(),
            }
        })?;
        let change_epoch = ProjectionEpoch::new(change_epoch.clone()).map_err(|error| {
            DirectProjectionTargetResolutionError::InvalidTarget {
                projector: self.projector.clone(),
                model: self.model.clone(),
                reason: error.to_string(),
            }
        })?;
        let topology = self.protocol_topology.clone().ok_or_else(|| {
            DirectProjectionTargetResolutionError::InvalidTarget {
                projector: self.projector.clone(),
                model: self.model.clone(),
                reason: "direct projection target was not bound to its complete compiled topology"
                    .into(),
            }
        })?;
        let codec = ProjectionScopeCodec::with_models(
            topology,
            [(self.schema.model_name.as_str(), self.schema)],
        )
        .map_err(
            |error| DirectProjectionTargetResolutionError::InvalidTarget {
                projector: self.projector.clone(),
                model: self.model.clone(),
                reason: error.to_string(),
            },
        )?;
        let partition_value = self
            .partition
            .as_ref()
            .map(|expression| {
                resolve_direct_projection_expression(
                    canonical_wire_input,
                    self,
                    "partition",
                    expression,
                )
            })
            .transpose()?;
        let partition = codec
            .encode_partition(partition_value.as_ref())
            .map_err(
                |error| DirectProjectionTargetResolutionError::InvalidTarget {
                    projector: self.projector.clone(),
                    model: self.model.clone(),
                    reason: error.to_string(),
                },
            )?;
        Ok(ResolvedDirectProjectionTarget {
            codec: Arc::new(codec),
            partition_value,
            partition,
            change_epoch,
            model: self.model.clone(),
            table: self.table.clone(),
            schema: self.schema,
            ownership: self.ownership.clone(),
        })
    }
}

/// Opaque compiler product attached to a typed projected command.
#[doc(hidden)]
pub struct CompiledDirectProjectionTarget<I, M>(
    CommandDirectProjectionTarget,
    PhantomData<fn(I) -> M>,
);

impl<I, M> CompiledDirectProjectionTarget<I, M> {
    /// Generated declarations may resolve the registered projection partition
    /// from one typed canonical input expression.
    #[doc(hidden)]
    pub fn partition<Wire>(mut self, partition: TypedEffectExpression<String, Wire>) -> Self
    where
        Wire: EffectWireCompatible<EffectWireString>,
    {
        self.0.partition = Some(partition.__into_ir());
        self
    }
}

pub(crate) fn compiled_direct_projection_target<I, M>(
    projector: &str,
    facts: &[String],
    models: &[String],
    projector_partition: &ProjectionPartitionSpec,
    change_epoch: Option<&str>,
) -> CompiledDirectProjectionTarget<I, M>
where
    M: RelationalReadModel + 'static,
{
    let schema = M::schema();
    let mut projected = CommandProjectedModel::new(TypeId::of::<M>(), schema);
    if let ProjectionPartitionSpec::Constant { value } = projector_partition {
        projected.partition = Some(EffectExpression::Constant {
            value: value.clone(),
        });
    }
    CompiledDirectProjectionTarget(
        projected.bind(
            projector,
            facts,
            models,
            projector_partition,
            change_epoch,
            vec![
                ProjectionModelOwnership::new(&schema.model_name, &schema.table_name)
                    .expect("validated relational schema has bounded model/table names"),
            ],
            None,
        ),
        PhantomData,
    )
}

pub(crate) struct ResolvedDirectProjectionTarget {
    codec: Arc<ProjectionScopeCodec>,
    partition_value: Option<serde_json::Value>,
    partition: ProjectionPartition,
    change_epoch: ProjectionEpoch,
    model: String,
    table: String,
    schema: &'static TableSchema,
    ownership: Vec<ProjectionModelOwnership>,
}

impl ResolvedDirectProjectionTarget {
    pub(crate) fn registration(&self) -> (&ProjectorTopologyId, &[ProjectionModelOwnership]) {
        (self.codec.topology(), &self.ownership)
    }

    fn seal(
        self,
        mutation: TableMutation,
        causation_id: &str,
    ) -> Result<SameTransactionProjectionBatch, ProjectionProtocolError> {
        let TableMutation::UpsertRow(row) = &mutation else {
            return Err(ProjectionProtocolError::InvalidBatch(
                "direct projection proof did not extract a full-row upsert".into(),
            ));
        };
        if row.schema != self.schema
            || row.schema.model_name != self.model
            || row.schema.table_name != self.table
        {
            return Err(ProjectionProtocolError::InvalidBatch(format!(
                "direct projection target `{}`/`{}` does not match the staged row",
                self.model, self.table
            )));
        }
        let scope = self
            .codec
            .encode_row_scope(
                self.codec.topology().name(),
                &self.model,
                self.partition_value.as_ref(),
                &row.key,
            )
            .map_err(|error| ProjectionProtocolError::InvalidBatch(error.to_string()))?;
        let ownership = ProjectionModelOwnership::new(self.model, self.table)?;
        SameTransactionProjectionBatch::single_upsert(
            self.codec.topology().clone(),
            self.partition,
            self.change_epoch,
            ownership,
            scope,
            mutation,
            causation_id,
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DirectProjectionTargetResolutionError {
    MissingInputPath {
        projector: String,
        model: String,
        target: String,
        path: Vec<String>,
    },
    TrustedPresetUnavailable {
        projector: String,
        model: String,
        target: String,
        preset: String,
    },
    InvalidConstant {
        projector: String,
        model: String,
        target: String,
        error: String,
    },
    InvalidTarget {
        projector: String,
        model: String,
        reason: String,
    },
}

impl std::fmt::Display for DirectProjectionTargetResolutionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingInputPath {
                projector,
                model,
                target,
                path,
            } => write!(
                formatter,
                "direct projection `{projector}`/`{model}` {target} references absent canonical input path `{}`",
                path.join("."),
            ),
            Self::TrustedPresetUnavailable {
                projector,
                model,
                target,
                preset,
            } => write!(
                formatter,
                "direct projection `{projector}`/`{model}` {target} uses unavailable trusted preset `{preset}`",
            ),
            Self::InvalidConstant {
                projector,
                model,
                target,
                error,
            } => write!(
                formatter,
                "direct projection `{projector}`/`{model}` {target} contains an invalid constant: {error}",
            ),
            Self::InvalidTarget {
                projector,
                model,
                reason,
            } => write!(
                formatter,
                "direct projection `{projector}`/`{model}` is invalid: {reason}"
            ),
        }
    }
}

impl std::error::Error for DirectProjectionTargetResolutionError {}

fn resolve_direct_projection_expression(
    canonical_wire_input: &serde_json::Value,
    target: &CommandDirectProjectionTarget,
    field: &str,
    expression: &EffectExpression,
) -> Result<serde_json::Value, DirectProjectionTargetResolutionError> {
    match expression {
        EffectExpression::Input { path } => {
            let mut value = canonical_wire_input;
            if path.is_empty() {
                return Err(DirectProjectionTargetResolutionError::MissingInputPath {
                    projector: target.projector.clone(),
                    model: target.model.clone(),
                    target: field.to_string(),
                    path: path.clone(),
                });
            }
            for segment in path {
                let Some(next) = value.as_object().and_then(|object| object.get(segment)) else {
                    return Err(DirectProjectionTargetResolutionError::MissingInputPath {
                        projector: target.projector.clone(),
                        model: target.model.clone(),
                        target: field.to_string(),
                        path: path.clone(),
                    });
                };
                value = next;
            }
            Ok(value.clone())
        }
        EffectExpression::Constant { value } => Ok(value.clone()),
        EffectExpression::Null => Ok(serde_json::Value::Null),
        EffectExpression::TrustedPreset { name } => Err(
            DirectProjectionTargetResolutionError::TrustedPresetUnavailable {
                projector: target.projector.clone(),
                model: target.model.clone(),
                target: field.to_string(),
                preset: name.clone(),
            },
        ),
        EffectExpression::InvalidConstant { error } => {
            Err(DirectProjectionTargetResolutionError::InvalidConstant {
                projector: target.projector.clone(),
                model: target.model.clone(),
                target: field.to_string(),
                error: error.clone(),
            })
        }
    }
}

/// Relationship identity used by link and unlink effects.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct EffectRelationship {
    pub source_model: String,
    pub field: String,
    pub target_model: String,
}

/// Closed portable command-effect operation set.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum CommandEffect {
    Upsert {
        model: String,
        key: EffectKey,
        fields: Vec<EffectFieldValue>,
    },
    Patch {
        model: String,
        key: EffectKey,
        fields: Vec<EffectFieldValue>,
    },
    Delete {
        model: String,
        key: EffectKey,
    },
    Link {
        relationship: EffectRelationship,
        source: EffectKey,
        target: EffectKey,
    },
    Unlink {
        relationship: EffectRelationship,
        source: EffectKey,
        target: EffectKey,
    },
    InvalidateModel {
        model: String,
    },
    InvalidateRelationship {
        relationship: EffectRelationship,
        source: EffectKey,
    },
}

/// What the client must do when the declared operations cannot prove safety.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CommandEffectFallback {
    /// No local invention: mark affected selections stale and revalidate.
    Revalidate,
}

/// Version-independent portable effect declaration attached to one command.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CommandEffects {
    pub operations: Vec<CommandEffect>,
    pub fallback: CommandEffectFallback,
}

impl CommandEffects {
    pub(crate) fn new(operations: impl IntoIterator<Item = CommandEffect>) -> Self {
        Self {
            operations: operations.into_iter().collect(),
            fallback: CommandEffectFallback::Revalidate,
        }
    }

    pub(crate) fn revalidate() -> Self {
        Self::new([])
    }

    pub(crate) fn canonicalize(&mut self) {
        for operation in &mut self.operations {
            match operation {
                CommandEffect::Upsert { fields, .. } | CommandEffect::Patch { fields, .. } => {
                    fields.sort_by(|left, right| left.field.cmp(&right.field));
                }
                CommandEffect::Delete { .. }
                | CommandEffect::Link { .. }
                | CommandEffect::Unlink { .. }
                | CommandEffect::InvalidateModel { .. }
                | CommandEffect::InvalidateRelationship { .. } => {}
            }
        }
    }

    fn invalid_constant_error(&self) -> Option<&str> {
        self.operations
            .iter()
            .find_map(|operation| match operation {
                CommandEffect::Upsert { key, fields, .. }
                | CommandEffect::Patch { key, fields, .. } => invalid_key_constant(key)
                    .or_else(|| fields.iter().find_map(invalid_field_constant)),
                CommandEffect::Delete { key, .. } => invalid_key_constant(key),
                CommandEffect::Link { source, target, .. }
                | CommandEffect::Unlink { source, target, .. } => {
                    invalid_key_constant(source).or_else(|| invalid_key_constant(target))
                }
                CommandEffect::InvalidateRelationship { source, .. } => {
                    invalid_key_constant(source)
                }
                CommandEffect::InvalidateModel { .. } => None,
            })
    }
}

fn invalid_expression_constant(expression: &EffectExpression) -> Option<&str> {
    match expression {
        EffectExpression::InvalidConstant { error } => Some(error),
        EffectExpression::Input { .. }
        | EffectExpression::TrustedPreset { .. }
        | EffectExpression::Constant { .. }
        | EffectExpression::Null => None,
    }
}

fn invalid_field_constant(field: &EffectFieldValue) -> Option<&str> {
    invalid_expression_constant(&field.value)
}

fn invalid_key_constant(key: &EffectKey) -> Option<&str> {
    key.fields.iter().find_map(invalid_field_constant)
}

fn invalid_confirmation_constant(confirmation: &CommandProjectionConfirmation) -> Option<&str> {
    invalid_key_constant(&confirmation.key).or_else(|| {
        confirmation
            .partition
            .as_ref()
            .and_then(invalid_expression_constant)
    })
}

/// Wire-shape proofs emitted by derives and erased after compatibility checks.
#[doc(hidden)]
pub struct EffectWireChecked;
#[doc(hidden)]
pub struct EffectWireLiteral;
#[doc(hidden)]
pub struct EffectWireString;
#[doc(hidden)]
pub struct EffectWireBoolean;
#[doc(hidden)]
pub struct EffectWireBigInt;
#[doc(hidden)]
pub struct EffectWireFloat;
#[doc(hidden)]
pub struct EffectWireJson;
#[doc(hidden)]
pub struct EffectWireBytea;
#[doc(hidden)]
pub struct EffectWireTimestamp;
#[doc(hidden)]
pub struct EffectWireList;
#[doc(hidden)]
pub struct EffectWireObject;
#[doc(hidden)]
pub struct EffectWireUnsupported;

/// Closed compile-time compatibility relation between GraphQL input wire
/// shapes and read-model scalar codecs.
#[doc(hidden)]
pub trait EffectWireCompatible<Target> {}

macro_rules! exact_effect_wire_compatibility {
    ($($wire:ty),+ $(,)?) => {
        $(impl EffectWireCompatible<$wire> for $wire {})+
    };
}

exact_effect_wire_compatibility!(
    EffectWireString,
    EffectWireBoolean,
    EffectWireBigInt,
    EffectWireFloat,
    EffectWireJson,
    EffectWireBytea,
    EffectWireTimestamp,
    EffectWireList,
    EffectWireObject,
    EffectWireUnsupported,
);

// JSON model columns deliberately accept a complete input container as their
// leaf value. Other scalar codecs never accept list/object wire shapes.
impl EffectWireCompatible<EffectWireJson> for EffectWireList {}
impl EffectWireCompatible<EffectWireJson> for EffectWireObject {}

// Constants and explicit null retain exact Rust value typing; Surface
// validation checks their serialized scalar/null representation.
impl<Target> EffectWireCompatible<Target> for EffectWireLiteral {}

/// Typed portable expression used only while constructing erased effect IR.
#[doc(hidden)]
pub struct TypedEffectExpression<T, Wire = EffectWireChecked> {
    expression: EffectExpression,
    _value: PhantomData<fn() -> (T, Wire)>,
}

impl<T, Wire> TypedEffectExpression<T, Wire> {
    #[doc(hidden)]
    pub(crate) fn __into_ir(self) -> EffectExpression {
        self.expression
    }

    fn erase_wire(self) -> TypedEffectExpression<T> {
        TypedEffectExpression {
            expression: self.expression,
            _value: PhantomData,
        }
    }
}

/// Marker implemented only by `GraphqlInput` derive output in normal use.
///
/// The trait must be public because derive output lives in downstream crates.
/// All marker metadata is still revalidated against the final command Surface;
/// hand-written implementations cannot bypass runtime structural checks.
#[doc(hidden)]
pub struct EffectRequired;

#[doc(hidden)]
pub struct EffectNullable;

/// Derive-owned classification for a field that is a nested input object and
/// may therefore appear before another segment in an effect input path.
#[doc(hidden)]
pub struct EffectInputObjectKind;

/// Derive-owned classification for scalar and list fields. These fields are
/// valid leaves but cannot be traversed by effect input paths.
#[doc(hidden)]
pub struct EffectInputTerminalKind;

#[doc(hidden)]
pub trait EffectInputPathKind {}

impl EffectInputPathKind for EffectInputObjectKind {}
impl EffectInputPathKind for EffectInputTerminalKind {}

/// Implemented only for the derive-owned nested-object classification.
#[doc(hidden)]
pub trait EffectInputDescendableKind: EffectInputPathKind {}

impl EffectInputDescendableKind for EffectInputObjectKind {}

#[doc(hidden)]
pub trait EffectPathNullability {
    type Applied<T>;
}

impl EffectPathNullability for EffectRequired {
    type Applied<T> = T;
}

impl EffectPathNullability for EffectNullable {
    type Applied<T> = Option<T>;
}

#[doc(hidden)]
pub trait CombineEffectNullability<Other: EffectPathNullability> {
    type Output: EffectPathNullability;
}

impl CombineEffectNullability<EffectRequired> for EffectRequired {
    type Output = EffectRequired;
}

impl CombineEffectNullability<EffectNullable> for EffectRequired {
    type Output = EffectNullable;
}

impl CombineEffectNullability<EffectRequired> for EffectNullable {
    type Output = EffectNullable;
}

impl CombineEffectNullability<EffectNullable> for EffectNullable {
    type Output = EffectNullable;
}

#[doc(hidden)]
pub trait EffectInputFieldMarker {
    type Input: 'static;
    type Value;
    type NonNullValue;
    type Nullability: EffectPathNullability;
    type PathKind: EffectInputPathKind;
    type Wire;
    /// Unwrapped object type used when descending through a nested input.
    type Nested: 'static;
    fn path() -> Vec<&'static str>;
}

/// Type-level composition of two derive-generated input-field markers.
#[doc(hidden)]
pub struct EffectInputPath<Outer, Inner>(PhantomData<fn(Outer) -> Inner>);

impl<Outer, Inner> EffectInputFieldMarker for EffectInputPath<Outer, Inner>
where
    Outer: EffectInputFieldMarker,
    Inner: EffectInputFieldMarker<Input = Outer::Nested>,
    Outer::PathKind: EffectInputDescendableKind,
    Outer::Nullability: CombineEffectNullability<Inner::Nullability>,
{
    type Input = Outer::Input;
    type Value = <<Outer::Nullability as CombineEffectNullability<
        Inner::Nullability,
    >>::Output as EffectPathNullability>::Applied<Inner::NonNullValue>;
    type NonNullValue = Inner::NonNullValue;
    type Nullability = <Outer::Nullability as CombineEffectNullability<Inner::Nullability>>::Output;
    type PathKind = Inner::PathKind;
    type Wire = Inner::Wire;
    type Nested = Inner::Nested;

    fn path() -> Vec<&'static str> {
        let mut path = Outer::path();
        path.extend(Inner::path());
        path
    }
}

/// Convert a derive-generated input marker into a typed portable expression.
#[doc(hidden)]
pub fn __effect_input<I, F>() -> TypedEffectExpression<F::Value, F::Wire>
where
    I: 'static,
    F: EffectInputFieldMarker<Input = I>,
{
    TypedEffectExpression {
        expression: EffectExpression::Input {
            path: F::path().into_iter().map(str::to_string).collect(),
        },
        _value: PhantomData,
    }
}

/// Wraps a serializer so every nested value is visited through the same
/// portable-JSON checks. This is intentionally one pass: even a stateful custom
/// `Serialize` implementation cannot validate one value and emit another.
struct StrictPortableJsonSerializer<S>(S);

struct StrictPortableJsonValue<'a, T: ?Sized>(&'a T);

impl<T> Serialize for StrictPortableJsonValue<'_, T>
where
    T: ?Sized + Serialize,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.0.serialize(StrictPortableJsonSerializer(serializer))
    }
}

macro_rules! delegate_portable_scalar {
    ($($method:ident($value:ty)),+ $(,)?) => {
        $(
            fn $method(self, value: $value) -> Result<Self::Ok, Self::Error> {
                self.0.$method(value)
            }
        )+
    };
}

impl<S> serde::Serializer for StrictPortableJsonSerializer<S>
where
    S: serde::Serializer,
{
    type Ok = S::Ok;
    type Error = S::Error;
    type SerializeSeq = StrictSerializeSeq<S::SerializeSeq>;
    type SerializeTuple = StrictSerializeTuple<S::SerializeTuple>;
    type SerializeTupleStruct = StrictSerializeTupleStruct<S::SerializeTupleStruct>;
    type SerializeTupleVariant = StrictSerializeTupleVariant<S::SerializeTupleVariant>;
    type SerializeMap = StrictSerializeMap<S::SerializeMap>;
    type SerializeStruct = StrictSerializeStruct<S::SerializeStruct>;
    type SerializeStructVariant = StrictSerializeStructVariant<S::SerializeStructVariant>;

    delegate_portable_scalar! {
        serialize_bool(bool),
        serialize_i8(i8),
        serialize_i16(i16),
        serialize_i32(i32),
        serialize_i64(i64),
        serialize_i128(i128),
        serialize_u8(u8),
        serialize_u16(u16),
        serialize_u32(u32),
        serialize_u64(u64),
        serialize_u128(u128),
        serialize_char(char),
    }

    fn serialize_f32(self, value: f32) -> Result<Self::Ok, Self::Error> {
        if !value.is_finite() {
            return Err(<S::Error as serde::ser::Error>::custom(
                "non-finite f32/f64 constants cannot be represented in portable JSON",
            ));
        }
        self.0.serialize_f32(value)
    }

    fn serialize_f64(self, value: f64) -> Result<Self::Ok, Self::Error> {
        if !value.is_finite() {
            return Err(<S::Error as serde::ser::Error>::custom(
                "non-finite f32/f64 constants cannot be represented in portable JSON",
            ));
        }
        self.0.serialize_f64(value)
    }

    fn serialize_str(self, value: &str) -> Result<Self::Ok, Self::Error> {
        self.0.serialize_str(value)
    }

    fn serialize_bytes(self, value: &[u8]) -> Result<Self::Ok, Self::Error> {
        self.0.serialize_bytes(value)
    }

    fn serialize_none(self) -> Result<Self::Ok, Self::Error> {
        self.0.serialize_none()
    }

    fn serialize_some<T>(self, value: &T) -> Result<Self::Ok, Self::Error>
    where
        T: ?Sized + Serialize,
    {
        self.0.serialize_some(&StrictPortableJsonValue(value))
    }

    fn serialize_unit(self) -> Result<Self::Ok, Self::Error> {
        self.0.serialize_unit()
    }

    fn serialize_unit_struct(self, name: &'static str) -> Result<Self::Ok, Self::Error> {
        self.0.serialize_unit_struct(name)
    }

    fn serialize_unit_variant(
        self,
        name: &'static str,
        variant_index: u32,
        variant: &'static str,
    ) -> Result<Self::Ok, Self::Error> {
        self.0.serialize_unit_variant(name, variant_index, variant)
    }

    fn serialize_newtype_struct<T>(
        self,
        name: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Self::Error>
    where
        T: ?Sized + Serialize,
    {
        self.0
            .serialize_newtype_struct(name, &StrictPortableJsonValue(value))
    }

    fn serialize_newtype_variant<T>(
        self,
        name: &'static str,
        variant_index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Self::Error>
    where
        T: ?Sized + Serialize,
    {
        self.0.serialize_newtype_variant(
            name,
            variant_index,
            variant,
            &StrictPortableJsonValue(value),
        )
    }

    fn serialize_seq(self, len: Option<usize>) -> Result<Self::SerializeSeq, Self::Error> {
        self.0.serialize_seq(len).map(StrictSerializeSeq)
    }

    fn serialize_tuple(self, len: usize) -> Result<Self::SerializeTuple, Self::Error> {
        self.0.serialize_tuple(len).map(StrictSerializeTuple)
    }

    fn serialize_tuple_struct(
        self,
        name: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleStruct, Self::Error> {
        self.0
            .serialize_tuple_struct(name, len)
            .map(StrictSerializeTupleStruct)
    }

    fn serialize_tuple_variant(
        self,
        name: &'static str,
        variant_index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleVariant, Self::Error> {
        self.0
            .serialize_tuple_variant(name, variant_index, variant, len)
            .map(StrictSerializeTupleVariant)
    }

    fn serialize_map(self, len: Option<usize>) -> Result<Self::SerializeMap, Self::Error> {
        self.0.serialize_map(len).map(StrictSerializeMap)
    }

    fn serialize_struct(
        self,
        name: &'static str,
        len: usize,
    ) -> Result<Self::SerializeStruct, Self::Error> {
        self.0
            .serialize_struct(name, len)
            .map(StrictSerializeStruct)
    }

    fn serialize_struct_variant(
        self,
        name: &'static str,
        variant_index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Self::SerializeStructVariant, Self::Error> {
        self.0
            .serialize_struct_variant(name, variant_index, variant, len)
            .map(StrictSerializeStructVariant)
    }

    fn is_human_readable(&self) -> bool {
        self.0.is_human_readable()
    }
}

struct StrictSerializeSeq<S>(S);

impl<S> SerializeSeq for StrictSerializeSeq<S>
where
    S: SerializeSeq,
{
    type Ok = S::Ok;
    type Error = S::Error;

    fn serialize_element<T>(&mut self, value: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + Serialize,
    {
        self.0.serialize_element(&StrictPortableJsonValue(value))
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        self.0.end()
    }
}

struct StrictSerializeTuple<S>(S);

impl<S> SerializeTuple for StrictSerializeTuple<S>
where
    S: SerializeTuple,
{
    type Ok = S::Ok;
    type Error = S::Error;

    fn serialize_element<T>(&mut self, value: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + Serialize,
    {
        self.0.serialize_element(&StrictPortableJsonValue(value))
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        self.0.end()
    }
}

struct StrictSerializeTupleStruct<S>(S);

impl<S> SerializeTupleStruct for StrictSerializeTupleStruct<S>
where
    S: SerializeTupleStruct,
{
    type Ok = S::Ok;
    type Error = S::Error;

    fn serialize_field<T>(&mut self, value: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + Serialize,
    {
        self.0.serialize_field(&StrictPortableJsonValue(value))
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        self.0.end()
    }
}

struct StrictSerializeTupleVariant<S>(S);

impl<S> SerializeTupleVariant for StrictSerializeTupleVariant<S>
where
    S: SerializeTupleVariant,
{
    type Ok = S::Ok;
    type Error = S::Error;

    fn serialize_field<T>(&mut self, value: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + Serialize,
    {
        self.0.serialize_field(&StrictPortableJsonValue(value))
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        self.0.end()
    }
}

struct StrictSerializeMap<S>(S);

impl<S> SerializeMap for StrictSerializeMap<S>
where
    S: SerializeMap,
{
    type Ok = S::Ok;
    type Error = S::Error;

    fn serialize_key<T>(&mut self, key: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + Serialize,
    {
        self.0.serialize_key(&StrictPortableJsonValue(key))
    }

    fn serialize_value<T>(&mut self, value: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + Serialize,
    {
        self.0.serialize_value(&StrictPortableJsonValue(value))
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        self.0.end()
    }
}

struct StrictSerializeStruct<S>(S);

impl<S> SerializeStruct for StrictSerializeStruct<S>
where
    S: SerializeStruct,
{
    type Ok = S::Ok;
    type Error = S::Error;

    fn serialize_field<T>(&mut self, key: &'static str, value: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + Serialize,
    {
        self.0.serialize_field(key, &StrictPortableJsonValue(value))
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        self.0.end()
    }
}

struct StrictSerializeStructVariant<S>(S);

impl<S> SerializeStructVariant for StrictSerializeStructVariant<S>
where
    S: SerializeStructVariant,
{
    type Ok = S::Ok;
    type Error = S::Error;

    fn serialize_field<T>(&mut self, key: &'static str, value: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + Serialize,
    {
        self.0.serialize_field(key, &StrictPortableJsonValue(value))
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        self.0.end()
    }
}

/// Serialize a deterministic value emitted by `command_effects!` while
/// retaining its Rust type for assignment checking. Serialization failures are
/// retained as private invalid IR and reported as configuration errors; a
/// declaration must never panic while it is being assembled.
#[doc(hidden)]
pub fn __effect_constant<T: Serialize>(value: T) -> TypedEffectExpression<T, EffectWireLiteral> {
    let expression = match StrictPortableJsonValue(&value).serialize(serde_json::value::Serializer)
    {
        Ok(value) => EffectExpression::Constant { value },
        Err(error) => EffectExpression::InvalidConstant {
            error: error.to_string(),
        },
    };
    TypedEffectExpression {
        expression,
        _value: PhantomData,
    }
}

/// Explicit nullable constant for optional model fields.
#[doc(hidden)]
pub fn __effect_null<T>() -> TypedEffectExpression<Option<T>, EffectWireLiteral> {
    TypedEffectExpression {
        expression: EffectExpression::Null,
        _value: PhantomData,
    }
}

/// Assignment compatibility implemented by framework expression types.
/// Non-null values may flow into nullable fields; nullable values never flow
/// into non-null fields. Key expressions remain exact and do not use this
/// conversion.
#[doc(hidden)]
pub trait EffectAssignmentExpression<Target> {
    type Wire;
    fn into_assignment(self) -> TypedEffectExpression<Target, Self::Wire>;
}

impl<T, Wire> EffectAssignmentExpression<T> for TypedEffectExpression<T, Wire> {
    type Wire = Wire;

    fn into_assignment(self) -> TypedEffectExpression<T, Wire> {
        self
    }
}

impl<T, Wire> EffectAssignmentExpression<Option<T>> for TypedEffectExpression<T, Wire> {
    type Wire = Wire;

    fn into_assignment(self) -> TypedEffectExpression<Option<T>, Wire> {
        TypedEffectExpression {
            expression: self.expression,
            _value: PhantomData,
        }
    }
}

/// Marker implemented by a `ReadModel` derive for one concrete model field.
#[doc(hidden)]
pub trait EffectModelFieldMarker {
    type Model: RelationalReadModel;
    type Value;
    type Wire;
    const FIELD: &'static str;
}

/// Marker implemented by a `ReadModel` derive for one relationship.
#[doc(hidden)]
pub trait EffectRelationshipMarker {
    type Source: RelationalReadModel;
    type Target: RelationalReadModel;
    const FIELD: &'static str;
}

/// Typed model key generated as a named struct by `#[derive(ReadModel)]`.
#[doc(hidden)]
pub struct TypedEffectKey<M> {
    key: EffectKey,
    _model: PhantomData<fn() -> M>,
}

/// Opaque, typed confirmation target created from a projector declaration.
///
/// The projector object and model key are reused directly, avoiding a second
/// application-maintained projector/model string join. Final Surface building
/// still validates the target against authorized projector topology.
#[doc(hidden)]
pub struct CompiledProjectionConfirmation<I>(CommandProjectionConfirmation, PhantomData<fn(I)>);

impl<I> CompiledProjectionConfirmation<I> {
    /// Partition the expected projector progress by a deterministic string/ID
    /// expression from the same command input.
    pub fn partition<Wire>(mut self, partition: TypedEffectExpression<String, Wire>) -> Self
    where
        Wire: EffectWireCompatible<EffectWireString>,
    {
        self.0.partition = Some(partition.__into_ir());
        self
    }
}

pub(crate) fn projection_confirmation<M: RelationalReadModel>(
    projector: &str,
    facts: &[String],
    models: &[String],
    partition: &ProjectionPartitionSpec,
    key: TypedEffectKey<M>,
) -> CommandProjectionConfirmation {
    CommandProjectionConfirmation {
        projector: projector.to_string(),
        model: M::schema().model_name.clone(),
        key: key.key,
        partition: match partition {
            ProjectionPartitionSpec::Constant { value } => Some(EffectExpression::Constant {
                value: value.clone(),
            }),
            ProjectionPartitionSpec::Unit | ProjectionPartitionSpec::InputPath { .. } => None,
        },
        projector_topology: ProjectorTopologyIdentity::new(projector, facts, models, partition),
        protocol_topology: None,
        schema: Some(M::schema()),
    }
}

pub(crate) fn compiled_projection_confirmation<I, M: RelationalReadModel>(
    projector: &str,
    facts: &[String],
    models: &[String],
    partition: &ProjectionPartitionSpec,
    key: TypedEffectKey<M>,
) -> CompiledProjectionConfirmation<I> {
    CompiledProjectionConfirmation(
        projection_confirmation(projector, facts, models, partition, key),
        PhantomData,
    )
}

/// Compile-checked, declaration-owned projection confirmation plan.
///
/// The input type parameter prevents a plan built for a lookalike input type
/// from being attached to a different command declaration.
pub struct CompiledConfirmationPlan<I>(Vec<CommandProjectionConfirmation>, PhantomData<fn(I)>);

#[doc(hidden)]
pub fn __command_confirmations<I>(
    confirmations: impl IntoIterator<Item = CompiledProjectionConfirmation<I>>,
) -> CompiledConfirmationPlan<I> {
    CompiledConfirmationPlan(
        confirmations
            .into_iter()
            .map(|confirmation| confirmation.0)
            .collect(),
        PhantomData,
    )
}

/// Opaque key field emitted by a `ReadModel` derive. Application code cannot
/// assemble raw effect IR through the public typed-command API.
#[doc(hidden)]
pub struct CompiledEffectKeyField<M>(EffectFieldValue, PhantomData<fn() -> M>);

#[doc(hidden)]
pub fn __effect_key_field<F>(
    value: TypedEffectExpression<F::Value>,
) -> CompiledEffectKeyField<F::Model>
where
    F: EffectModelFieldMarker,
{
    CompiledEffectKeyField(
        EffectFieldValue {
            field: F::FIELD.to_string(),
            value: value.__into_ir(),
        },
        PhantomData,
    )
}

/// Prove one generated key expression's wire compatibility, then erase the
/// proof so derive-generated composite key structs need no wire generics.
#[doc(hidden)]
pub fn __effect_key_assignment<F, Wire>(
    value: TypedEffectExpression<F::Value, Wire>,
) -> TypedEffectExpression<F::Value>
where
    F: EffectModelFieldMarker,
    Wire: EffectWireCompatible<F::Wire>,
{
    value.erase_wire()
}

/// Assemble a typed model key from derive-generated field markers. The final
/// Surface validator additionally requires the exact ordered primary key.
#[doc(hidden)]
pub fn __effect_key<M: RelationalReadModel>(
    fields: Vec<CompiledEffectKeyField<M>>,
) -> TypedEffectKey<M> {
    TypedEffectKey {
        key: EffectKey {
            fields: fields.into_iter().map(|field| field.0).collect(),
        },
        _model: PhantomData,
    }
}

/// Typed relationship marker generated by `#[derive(ReadModel)]`.
#[doc(hidden)]
pub struct TypedEffectRelationship<S, T> {
    relationship: EffectRelationship,
    _models: PhantomData<fn(S) -> T>,
}

impl<S, T> TypedEffectRelationship<S, T> {}

/// Convert a derive-generated relationship marker into opaque typed IR.
#[doc(hidden)]
pub fn __effect_relationship<R>() -> TypedEffectRelationship<R::Source, R::Target>
where
    R: EffectRelationshipMarker,
{
    TypedEffectRelationship {
        relationship: EffectRelationship {
            source_model: R::Source::schema().model_name.clone(),
            field: R::FIELD.to_string(),
            target_model: R::Target::schema().model_name.clone(),
        },
        _models: PhantomData,
    }
}

/// Type-checked field assignment helper used by the effect proc macro.
#[doc(hidden)]
pub fn __effect_assignment<F, E>(value: E) -> CompiledEffectFieldValue<F::Model>
where
    F: EffectModelFieldMarker,
    E: EffectAssignmentExpression<F::Value>,
    E::Wire: EffectWireCompatible<F::Wire>,
{
    let value = value.into_assignment();
    CompiledEffectFieldValue(
        EffectFieldValue {
            field: F::FIELD.to_string(),
            value: value.__into_ir(),
        },
        PhantomData,
    )
}

/// Opaque type-checked field assignment emitted by `command_effects!`.
#[doc(hidden)]
pub struct CompiledEffectFieldValue<M>(EffectFieldValue, PhantomData<fn() -> M>);

/// Opaque compiled effect operation. Only generated typed helpers can produce
/// one; raw IR is not accepted by [`TypedCommand::effects`].
#[doc(hidden)]
pub struct CompiledEffectOperation(CommandEffect);

/// One compile-checked generated canonical-input default.
#[doc(hidden)]
pub struct CompiledInputDefault<I>(CommandInputDefault, PhantomData<fn(I)>);

/// Declaration-owned generated defaults for one exact command input type.
pub struct CompiledInputDefaults<I>(Vec<CommandInputDefault>, PhantomData<fn(I)>);

#[doc(hidden)]
pub fn __input_default_uuid_v7<I, F>() -> CompiledInputDefault<I>
where
    I: 'static,
    F: EffectInputFieldMarker<Input = I, Value = String>,
{
    CompiledInputDefault(
        CommandInputDefault {
            path: F::path().into_iter().map(str::to_string).collect(),
            generator: InputDefaultGenerator::UuidV7,
        },
        PhantomData,
    )
}

#[doc(hidden)]
pub fn __input_default_ulid<I, F>() -> CompiledInputDefault<I>
where
    I: 'static,
    F: EffectInputFieldMarker<Input = I, Value = String>,
{
    CompiledInputDefault(
        CommandInputDefault {
            path: F::path().into_iter().map(str::to_string).collect(),
            generator: InputDefaultGenerator::Ulid,
        },
        PhantomData,
    )
}

#[doc(hidden)]
pub fn __command_input_defaults<I>(
    defaults: impl IntoIterator<Item = CompiledInputDefault<I>>,
) -> CompiledInputDefaults<I> {
    CompiledInputDefaults(
        defaults.into_iter().map(|default| default.0).collect(),
        PhantomData,
    )
}

/// Opaque, compile-checked effect declaration returned by `command_effects!`.
pub struct CompiledCommandEffects<I>(CommandEffects, PhantomData<fn(I)>);

#[doc(hidden)]
pub fn __command_effects<I>(
    operations: impl IntoIterator<Item = CompiledEffectOperation>,
) -> CompiledCommandEffects<I> {
    CompiledCommandEffects(
        CommandEffects::new(operations.into_iter().map(|operation| operation.0)),
        PhantomData,
    )
}

/// Type-checked upsert helper used by the effect proc macro.
#[doc(hidden)]
pub fn __effect_upsert<M: RelationalReadModel>(
    key: TypedEffectKey<M>,
    fields: Vec<CompiledEffectFieldValue<M>>,
) -> CompiledEffectOperation {
    CompiledEffectOperation(CommandEffect::Upsert {
        model: M::schema().model_name.clone(),
        key: key.key,
        fields: fields.into_iter().map(|field| field.0).collect(),
    })
}

/// Type-checked patch helper used by the effect proc macro.
#[doc(hidden)]
pub fn __effect_patch<M: RelationalReadModel>(
    key: TypedEffectKey<M>,
    fields: Vec<CompiledEffectFieldValue<M>>,
) -> CompiledEffectOperation {
    CompiledEffectOperation(CommandEffect::Patch {
        model: M::schema().model_name.clone(),
        key: key.key,
        fields: fields.into_iter().map(|field| field.0).collect(),
    })
}

/// Type-checked delete helper used by the effect proc macro.
#[doc(hidden)]
pub fn __effect_delete<M: RelationalReadModel>(key: TypedEffectKey<M>) -> CompiledEffectOperation {
    CompiledEffectOperation(CommandEffect::Delete {
        model: M::schema().model_name.clone(),
        key: key.key,
    })
}

/// Type-checked link helper used by the effect proc macro.
#[doc(hidden)]
pub fn __effect_link<S: RelationalReadModel, T: RelationalReadModel>(
    relationship: TypedEffectRelationship<S, T>,
    source: TypedEffectKey<S>,
    target: TypedEffectKey<T>,
) -> CompiledEffectOperation {
    CompiledEffectOperation(CommandEffect::Link {
        relationship: relationship.relationship,
        source: source.key,
        target: target.key,
    })
}

/// Type-checked unlink helper used by the effect proc macro.
#[doc(hidden)]
pub fn __effect_unlink<S: RelationalReadModel, T: RelationalReadModel>(
    relationship: TypedEffectRelationship<S, T>,
    source: TypedEffectKey<S>,
    target: TypedEffectKey<T>,
) -> CompiledEffectOperation {
    CompiledEffectOperation(CommandEffect::Unlink {
        relationship: relationship.relationship,
        source: source.key,
        target: target.key,
    })
}

/// Type-checked model invalidation helper used by the effect proc macro.
#[doc(hidden)]
pub fn __effect_invalidate_model<M: RelationalReadModel>() -> CompiledEffectOperation {
    CompiledEffectOperation(CommandEffect::InvalidateModel {
        model: M::schema().model_name.clone(),
    })
}

/// Type-checked relationship invalidation helper used by the effect proc macro.
#[doc(hidden)]
pub fn __effect_invalidate_relationship<S: RelationalReadModel, T: RelationalReadModel>(
    relationship: TypedEffectRelationship<S, T>,
    source: TypedEffectKey<S>,
) -> CompiledEffectOperation {
    CompiledEffectOperation(CommandEffect::InvalidateRelationship {
        relationship: relationship.relationship,
        source: source.key,
    })
}

/// Stable erased metadata shared by the executable service and GraphQL engine.
#[derive(Clone, Debug)]
pub(crate) struct TypedCommandContract {
    pub name: String,
    pub field_name: String,
    pub roles: Vec<String>,
    pub input: GraphqlTypeDef,
    pub output: GraphqlTypeDef,
    pub input_type_id: TypeId,
    pub output_type_id: TypeId,
    pub consistency: CommandConsistency,
    pub input_defaults: Vec<CommandInputDefault>,
    pub effects: CommandEffects,
    pub confirmations: Vec<CommandProjectionConfirmation>,
    /// Present automatically for `Projected<M>` before Surface ownership is
    /// resolved. This never requires an application declaration.
    pub projected_model: Option<CommandProjectedModel>,
    pub direct_projection: Option<CommandDirectProjectionTarget>,
}

impl TypedCommandContract {
    /// Stable per-route identity used in the command ledger fingerprint.
    ///
    /// This is intentionally distinct from the service inventory digest: a
    /// deployment may add unrelated routes without invalidating safe retries
    /// for this command. The explicit domain/version prevents this byte digest
    /// from aliasing any other SHA-256 use in the framework.
    pub(crate) fn fingerprint_bytes(&self) -> [u8; 32] {
        let mut digest = Sha256::new();
        digest.update(b"distributed.typed-command-contract.v1");
        digest.update([0]);
        digest.update(
            serde_json::to_vec(&self.canonical_value())
                .expect("canonical typed command contract serialization cannot fail"),
        );
        digest.finalize().into()
    }

    pub(crate) fn canonical_value(&self) -> serde_json::Value {
        let mut roles = self.roles.clone();
        roles.sort();
        roles.dedup();
        let mut effects = self.effects.clone();
        effects.canonicalize();
        let mut input_defaults = self.input_defaults.clone();
        input_defaults.sort_by(|left, right| left.path.cmp(&right.path));
        let mut confirmations = self.confirmations.clone();
        confirmations.sort_by(|left, right| {
            serde_json::to_string(&left.canonical_value())
                .expect("confirmation IR serialization cannot fail")
                .cmp(
                    &serde_json::to_string(&right.canonical_value())
                        .expect("confirmation IR serialization cannot fail"),
                )
        });
        let confirmations = confirmations
            .iter()
            .map(CommandProjectionConfirmation::canonical_value)
            .collect::<Vec<_>>();
        let direct_projection = self
            .direct_projection
            .as_ref()
            .map(CommandDirectProjectionTarget::canonical_value);
        let projected_model = self
            .projected_model
            .as_ref()
            .map(CommandProjectedModel::canonical_value);
        serde_json::json!({
            "name": self.name,
            "field_name": self.field_name,
            "roles": roles,
            "input": canonical_graphql_type(&self.input),
            "output": canonical_graphql_type(&self.output),
            "consistency": self.consistency,
            "input_defaults": input_defaults,
            "effects": effects,
            "confirmations": confirmations,
            "projected_model": projected_model,
            "direct_projection": direct_projection,
        })
    }

    /// Resolve the finite declaration-owned projection plan from the exact
    /// canonical GraphQL wire input retained beside the decoded command.
    ///
    /// Resolution is pure and must run before commit I/O. Confirmation and key
    /// field order are retained exactly as declared. Input values and constants
    /// are cloned without a Rust DTO or SQL codec round trip, while explicit
    /// null remains distinguishable from an undeclared partition.
    pub(crate) fn resolve_projection_obligations(
        &self,
        canonical_wire_input: &serde_json::Value,
    ) -> Result<Vec<ResolvedProjectionObligation>, ProjectionObligationResolutionError> {
        self.confirmations
            .iter()
            .map(|confirmation| {
                let fields = confirmation
                    .key
                    .fields
                    .iter()
                    .map(|field| {
                        let target = format!("key field `{}`", field.field);
                        resolve_projection_obligation_expression(
                            canonical_wire_input,
                            confirmation,
                            &target,
                            &field.value,
                        )
                        .map(|value| ResolvedProjectionKeyField {
                            field: field.field.clone(),
                            value,
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let partition = confirmation
                    .partition
                    .as_ref()
                    .map(|expression| {
                        resolve_projection_obligation_expression(
                            canonical_wire_input,
                            confirmation,
                            "partition",
                            expression,
                        )
                    })
                    .transpose()?;
                let key = ResolvedProjectionKey { fields };
                let topology = confirmation.protocol_topology.clone().ok_or_else(|| {
                    ProjectionObligationResolutionError::InvalidBinding {
                        projector: confirmation.projector.clone(),
                        model: confirmation.model.clone(),
                        reason: "missing compiled topology identity".into(),
                    }
                })?;
                let schema = confirmation.schema.ok_or_else(|| {
                    ProjectionObligationResolutionError::InvalidBinding {
                        projector: confirmation.projector.clone(),
                        model: confirmation.model.clone(),
                        reason: "missing retained relational schema".into(),
                    }
                })?;
                let codec = ProjectionScopeCodec::with_models(
                    topology,
                    [(schema.model_name.as_str(), schema)],
                )
                .map_err(|error| {
                    ProjectionObligationResolutionError::InvalidBinding {
                        projector: confirmation.projector.clone(),
                        model: confirmation.model.clone(),
                        reason: error.to_string(),
                    }
                })?;
                let scope = codec
                    .encode_resolved_obligation_scope(
                        &confirmation.projector,
                        &confirmation.model,
                        &key,
                        partition.as_ref(),
                    )
                    .map_err(
                        |error| ProjectionObligationResolutionError::InvalidBinding {
                            projector: confirmation.projector.clone(),
                            model: confirmation.model.clone(),
                            reason: error.to_string(),
                        },
                    )?;
                Ok(ResolvedProjectionObligation {
                    projector: confirmation.projector.clone(),
                    model: confirmation.model.clone(),
                    key,
                    partition,
                    scope,
                })
            })
            .collect()
    }

    pub(crate) fn resolve_direct_projection_target(
        &self,
        canonical_wire_input: &serde_json::Value,
    ) -> Result<Option<ResolvedDirectProjectionTarget>, DirectProjectionTargetResolutionError> {
        self.direct_projection
            .as_ref()
            .map(|target| target.resolve(canonical_wire_input))
            .transpose()
    }

    /// Prove that every finite asynchronous confirmation can be driven by a
    /// fact staged in the durable outbox. Aggregate event records intentionally
    /// do not count: they are write-side history and have no publication path.
    fn validate_outbox_fact_coverage(
        &self,
        outbox_messages: &[OutboxMessage],
    ) -> Result<(), CommandCommitProofError> {
        if self.consistency == CommandConsistency::Fact && self.confirmations.is_empty() {
            return Err(CommandCommitProofError::FactHasNoConfirmations);
        }

        let staged_facts = outbox_messages
            .iter()
            .filter(|message| message.destination.is_none() && message.is_pending())
            .map(|message| message.event_type.clone())
            .collect::<BTreeSet<_>>();
        for confirmation in &self.confirmations {
            if confirmation
                .projector_topology
                .facts
                .iter()
                .any(|fact| staged_facts.contains(fact))
            {
                continue;
            }
            return Err(CommandCommitProofError::UnreachableConfirmation {
                projector: confirmation.projector.clone(),
                expected_facts: confirmation.projector_topology.facts.clone(),
                staged_facts: staged_facts.iter().cloned().collect(),
            });
        }
        Ok(())
    }
}

fn resolve_projection_obligation_expression(
    canonical_wire_input: &serde_json::Value,
    confirmation: &CommandProjectionConfirmation,
    target: &str,
    expression: &EffectExpression,
) -> Result<serde_json::Value, ProjectionObligationResolutionError> {
    match expression {
        EffectExpression::Input { path } => {
            let mut value = canonical_wire_input;
            if path.is_empty() {
                return Err(ProjectionObligationResolutionError::MissingInputPath {
                    projector: confirmation.projector.clone(),
                    model: confirmation.model.clone(),
                    target: target.to_string(),
                    path: path.clone(),
                });
            }
            for segment in path {
                let Some(next) = value.as_object().and_then(|object| object.get(segment)) else {
                    return Err(ProjectionObligationResolutionError::MissingInputPath {
                        projector: confirmation.projector.clone(),
                        model: confirmation.model.clone(),
                        target: target.to_string(),
                        path: path.clone(),
                    });
                };
                value = next;
            }
            Ok(value.clone())
        }
        EffectExpression::Constant { value } => Ok(value.clone()),
        EffectExpression::Null => Ok(serde_json::Value::Null),
        EffectExpression::TrustedPreset { name } => Err(
            ProjectionObligationResolutionError::TrustedPresetUnavailable {
                projector: confirmation.projector.clone(),
                model: confirmation.model.clone(),
                target: target.to_string(),
                preset: name.clone(),
            },
        ),
        EffectExpression::InvalidConstant { error } => {
            Err(ProjectionObligationResolutionError::InvalidConstant {
                projector: confirmation.projector.clone(),
                model: confirmation.model.clone(),
                target: target.to_string(),
                error: error.clone(),
            })
        }
    }
}

fn canonical_graphql_type(definition: &GraphqlTypeDef) -> serde_json::Value {
    let mut fields = definition.fields.iter().collect::<Vec<_>>();
    fields.sort_by(|left, right| left.name.cmp(&right.name));
    serde_json::json!({
        "name": definition.name,
        "fields": fields.into_iter().map(|field| serde_json::json!({
            "name": field.name,
            "type_name": field.type_name,
            "nullable": field.nullable,
            "list": field.list,
            "item_nullable": field.item_nullable,
            "nested": field.nested.as_deref().map(canonical_graphql_type),
        })).collect::<Vec<_>>(),
    })
}

/// Stable command inventory identity shared by a service and GraphQL engine.
///
/// The digest covers canonical wire structure while the non-serializable
/// `TypeId` pairs prove that both sides were built from the exact Rust input
/// and output types, not merely lookalike GraphQL shapes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TypedServiceCommandBinding {
    pub service_id: String,
    pub structural_fingerprint: String,
    pub types: BTreeMap<String, (TypeId, TypeId)>,
}

impl TypedServiceCommandBinding {
    pub(crate) fn from_contracts(
        service_id: &str,
        contracts: &[TypedCommandContract],
    ) -> Result<Self, String> {
        if service_id.trim().is_empty() {
            return Err("typed command inventory requires a non-empty service ID".into());
        }

        let mut seen = BTreeSet::new();
        let mut ordered = contracts.iter().collect::<Vec<_>>();
        ordered.sort_by(|left, right| left.name.cmp(&right.name));
        let mut types = BTreeMap::new();
        let mut canonical = Vec::with_capacity(ordered.len());
        for contract in ordered {
            if contract.name.trim().is_empty() {
                return Err("typed command id must not be empty".into());
            }
            if !seen.insert(contract.name.clone()) {
                return Err(format!(
                    "duplicate typed command declaration for `{}`",
                    contract.name
                ));
            }
            if contract.input.type_id != Some(contract.input_type_id) {
                return Err(format!(
                    "typed command `{}` input GraphQL metadata is missing or has a different Rust TypeId",
                    contract.name
                ));
            }
            if contract.output.type_id != Some(contract.output_type_id) {
                return Err(format!(
                    "typed command `{}` output GraphQL metadata is missing or has a different Rust TypeId",
                    contract.name
                ));
            }
            match contract.consistency {
                CommandConsistency::Fact if contract.confirmations.is_empty() => {
                    return Err(format!(
                        "typed fact command `{}` must declare at least one expected projector confirmation",
                        contract.name
                    ));
                }
                CommandConsistency::Projected if !contract.confirmations.is_empty() => {
                    return Err(format!(
                        "typed projected command `{}` cannot declare asynchronous projector confirmations",
                        contract.name
                    ));
                }
                CommandConsistency::Projected if contract.projected_model.is_none() => {
                    return Err(format!(
                        "typed projected command `{}` is missing its compiler-retained relational model",
                        contract.name
                    ));
                }
                CommandConsistency::Accepted | CommandConsistency::Fact
                    if contract.projected_model.is_some()
                        || contract.direct_projection.is_some() =>
                {
                    return Err(format!(
                        "typed non-projected command `{}` cannot carry direct projection metadata",
                        contract.name
                    ));
                }
                CommandConsistency::Fact
                | CommandConsistency::Accepted
                | CommandConsistency::Projected => {}
            }
            if let Some(projected) = &contract.projected_model {
                if projected.output_type_id != contract.output_type_id {
                    return Err(format!(
                        "typed projected command `{}` retained model has a different Rust output type",
                        contract.name
                    ));
                }
                if projected.model != contract.output.name {
                    return Err(format!(
                        "typed projected command `{}` retained model `{}` differs from output `{}`",
                        contract.name, projected.model, contract.output.name
                    ));
                }
            }
            if let Some(target) = &contract.direct_projection {
                if target.output_type_id != contract.output_type_id {
                    return Err(format!(
                        "typed projected command `{}` direct target has a different Rust output type",
                        contract.name
                    ));
                }
                if target.model != contract.output.name {
                    return Err(format!(
                        "typed projected command `{}` direct target model `{}` differs from output `{}`",
                        contract.name, target.model, contract.output.name
                    ));
                }
                let Some(change_epoch) = target.change_epoch.as_deref() else {
                    return Err(format!(
                        "typed projected command `{}` direct target has no registered change-log epoch",
                        contract.name
                    ));
                };
                ProjectionEpoch::new(change_epoch).map_err(|error| {
                    format!(
                        "typed projected command `{}` direct target change epoch is invalid: {error}",
                        contract.name
                    )
                })?;
            }
            if let Some(error) = contract
                .effects
                .invalid_constant_error()
                .or_else(|| {
                    contract
                        .confirmations
                        .iter()
                        .find_map(invalid_confirmation_constant)
                })
                .or_else(|| {
                    contract
                        .direct_projection
                        .as_ref()
                        .and_then(|target| target.partition.as_ref())
                        .and_then(invalid_expression_constant)
                })
                .or_else(|| {
                    contract
                        .projected_model
                        .as_ref()
                        .and_then(|model| model.partition.as_ref())
                        .and_then(invalid_expression_constant)
                })
            {
                return Err(format!(
                    "typed command `{}` constant effect value failed to serialize: {error}",
                    contract.name
                ));
            }
            let mut confirmations = BTreeSet::new();
            for confirmation in &contract.confirmations {
                let canonical = serde_json::to_string(&confirmation.canonical_value())
                    .expect("confirmation IR serialization cannot fail");
                if !confirmations.insert(canonical) {
                    return Err(format!(
                        "typed command `{}` repeats an expected projector confirmation",
                        contract.name
                    ));
                }
            }
            types.insert(
                contract.name.clone(),
                (contract.input_type_id, contract.output_type_id),
            );
            canonical.push(contract.canonical_value());
        }

        let bytes = serde_json::to_vec(&serde_json::json!({
            "service_id": service_id,
            "commands": canonical,
        }))
        .expect("serializing canonical command inventory cannot fail");
        Ok(Self {
            service_id: service_id.to_string(),
            structural_fingerprint: format!("sha256:{:x}", Sha256::digest(bytes)),
            types,
        })
    }
}

/// A typed command declaration registered together with its executable handler.
pub struct TypedCommand<I, K: CommandOutcome> {
    route_name: &'static str,
    contract: TypedCommandContract,
    _types: PhantomData<fn(I) -> K>,
}

impl<I, K: CommandOutcome> Clone for TypedCommand<I, K> {
    fn clone(&self) -> Self {
        Self {
            route_name: self.route_name,
            contract: self.contract.clone(),
            _types: PhantomData,
        }
    }
}

/// Begin a typed command declaration.
pub fn typed_command<I, K>(name: &'static str) -> TypedCommand<I, K>
where
    I: GraphqlInputType + DeserializeOwned + Send + 'static,
    K: CommandOutcome,
{
    let route_name = name;
    let name = route_name.to_string();
    let field_name = name
        .chars()
        .map(|character| match character {
            '.' | '-' => '_',
            other => other,
        })
        .collect();
    let input = I::graphql_type();
    let output = K::Payload::graphql_type();
    let projected_model = K::__projected_model()
        .map(|(output_type_id, schema)| CommandProjectedModel::new(output_type_id, schema));
    TypedCommand {
        route_name,
        contract: TypedCommandContract {
            name,
            field_name,
            roles: Vec::new(),
            input,
            output,
            input_type_id: TypeId::of::<I>(),
            output_type_id: TypeId::of::<K::Payload>(),
            consistency: K::CONSISTENCY,
            input_defaults: Vec::new(),
            effects: CommandEffects::revalidate(),
            confirmations: Vec::new(),
            projected_model,
            direct_projection: None,
        },
        _types: PhantomData,
    }
}

impl<I, K: CommandOutcome> TypedCommand<I, K> {
    pub fn field_name(mut self, field_name: impl Into<String>) -> Self {
        self.contract.field_name = field_name.into();
        self
    }

    pub fn roles(mut self, roles: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.contract.roles = roles.into_iter().map(Into::into).collect();
        self.contract.roles.sort();
        self.contract.roles.dedup();
        self
    }

    pub fn effects(mut self, effects: CompiledCommandEffects<I>) -> Self {
        self.contract.effects = effects.0;
        self.contract.effects.canonicalize();
        self
    }

    /// Declare values generated once into the canonical command input before
    /// dispatch. Effects and confirmations must reference the finalized input
    /// field rather than invoking a generator independently.
    pub fn input_defaults(mut self, defaults: CompiledInputDefaults<I>) -> Self {
        self.contract.input_defaults = defaults.0;
        self.contract
            .input_defaults
            .sort_by(|left, right| left.path.cmp(&right.path));
        self
    }

    /// Declare the finite projector/model/key progress that confirms this fact.
    /// `Fact<_>` commands require at least one confirmation. `Accepted<_>` may
    /// omit the plan (terminal accepted) or provide one (pending projection).
    /// `Projected<_>` commands cannot carry asynchronous confirmations.
    pub fn confirmations(mut self, confirmations: CompiledConfirmationPlan<I>) -> Self {
        self.contract.confirmations = confirmations.0;
        self
    }

    pub fn name(&self) -> &str {
        &self.contract.name
    }

    pub fn consistency(&self) -> CommandConsistency {
        self.contract.consistency
    }

    #[cfg(test)]
    pub(crate) fn into_contract(self) -> TypedCommandContract {
        self.contract
    }

    pub(crate) fn into_parts(self) -> (&'static str, TypedCommandContract) {
        (self.route_name, self.contract)
    }
}

impl<I, M> TypedCommand<I, Projected<M>>
where
    I: GraphqlInputType + DeserializeOwned + Send + 'static,
    M: GraphqlOutputType + RelationalReadModel + Serialize + Send + Sync + 'static,
{
    /// Attach compiler-generated direct projection ownership metadata.
    ///
    /// Application handlers do not call this; generated service inventory
    /// binds it from the registered projector declaration and handlers retain
    /// the `context.projected(view)` API.
    #[doc(hidden)]
    pub fn __direct_projection(mut self, target: CompiledDirectProjectionTarget<I, M>) -> Self {
        if let Some(projected) = &mut self.contract.projected_model {
            projected.partition = target.0.partition.clone();
        }
        self.contract.direct_projection = Some(target.0);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graphql::{GraphqlTypeDef, GraphqlTypeField};
    use crate::table::{ColumnType, PrimaryKey, TableColumn, TableKind};
    use serde::Deserialize;

    #[allow(dead_code)]
    #[derive(Deserialize)]
    struct Input {
        id: String,
    }

    impl GraphqlInputType for Input {
        fn graphql_type() -> GraphqlTypeDef {
            GraphqlTypeDef::new(
                "Input",
                vec![GraphqlTypeField {
                    name: "id".into(),
                    type_name: "String".into(),
                    nullable: false,
                    list: false,
                    item_nullable: false,
                    nested: None,
                }],
            )
            .with_type_id(TypeId::of::<Self>())
        }
    }

    #[derive(Serialize)]
    struct Payload {
        id: String,
    }

    impl GraphqlOutputType for Payload {
        fn graphql_type() -> GraphqlTypeDef {
            GraphqlTypeDef::new(
                "Payload",
                vec![GraphqlTypeField {
                    name: "id".into(),
                    type_name: "String".into(),
                    nullable: false,
                    list: false,
                    item_nullable: false,
                    nested: None,
                }],
            )
            .with_type_id(TypeId::of::<Self>())
        }
    }

    #[test]
    fn preparation_serializes_and_retains_the_typed_payload_until_commit() {
        let prepared = PreparedCommand::<Fact<Payload>>::prepare(Payload {
            id: "todo-1".into(),
        })
        .unwrap();
        assert_eq!(prepared.consistency(), CommandConsistency::Fact);
        assert_eq!(prepared.serialized_payload()["id"], "todo-1");
        let (committed, serialized) = prepared.finalize_after_commit();
        assert_eq!(committed.payload().id, "todo-1");
        assert_eq!(serialized["id"], "todo-1");
    }

    fn confirmation_with_facts(facts: &[&str]) -> CommandProjectionConfirmation {
        CommandProjectionConfirmation {
            projector: "project_todos".into(),
            model: "TodoView".into(),
            key: EffectKey { fields: Vec::new() },
            partition: None,
            projector_topology: ProjectorTopologyIdentity::new(
                "project_todos",
                &facts
                    .iter()
                    .map(|fact| (*fact).to_string())
                    .collect::<Vec<_>>(),
                &["TodoView".into()],
                &ProjectionPartitionSpec::unit(),
            ),
            protocol_topology: None,
            schema: None,
        }
    }

    #[test]
    fn confirmation_inventory_matches_the_bounded_status_batch() {
        let maximum = crate::projection_protocol::MAX_PROJECTION_EVIDENCE_BATCH_ITEMS;
        validate_projection_confirmation_count("todo.update", maximum)
            .expect("the adapter's exact maximum must be accepted");
        let error = validate_projection_confirmation_count("todo.update", maximum + 1)
            .expect_err("one more confirmation must fail before service traffic");
        assert!(error.contains(&format!("maximum is {maximum}")), "{error}");
    }

    fn confirmation_with_key(
        projector: &str,
        fields: impl IntoIterator<Item = (&'static str, EffectExpression)>,
        partition: Option<EffectExpression>,
    ) -> CommandProjectionConfirmation {
        let fields = fields
            .into_iter()
            .map(|(field, value)| EffectFieldValue {
                field: field.into(),
                value,
            })
            .collect::<Vec<_>>();
        let columns = fields
            .iter()
            .map(|field| TableColumn {
                primary_key: true,
                ..TableColumn::new(&field.field, &field.field, ColumnType::Json)
            })
            .collect::<Vec<_>>();
        let primary_key = fields.iter().map(|field| field.field.as_str());
        let schema = Box::leak(Box::new(TableSchema {
            model_name: "TodoView".into(),
            table_name: "todo_views".into(),
            columns,
            primary_key: PrimaryKey::new(primary_key),
            version_column: None,
            foreign_keys: Vec::new(),
            indexes: Vec::new(),
            relationships: Vec::new(),
            kind: TableKind::ReadModel,
        }));
        CommandProjectionConfirmation {
            projector: projector.into(),
            model: "TodoView".into(),
            key: EffectKey { fields },
            partition,
            projector_topology: ProjectorTopologyIdentity::new(
                projector,
                &["todo.changed".into()],
                &["TodoView".into()],
                &ProjectionPartitionSpec::unit(),
            ),
            protocol_topology: Some(ProjectorTopologyId::new(1, projector, [3; 32]).unwrap()),
            schema: Some(schema),
        }
    }

    #[test]
    fn projection_obligations_resolve_nested_canonical_wire_paths_in_declaration_order() {
        let mut contract = typed_command::<Input, Accepted<Payload>>("todo.update").into_contract();
        contract.confirmations = vec![
            confirmation_with_key(
                "project_second",
                [
                    (
                        "tenant_id",
                        EffectExpression::Input {
                            path: vec!["scope".into(), "tenantId".into()],
                        },
                    ),
                    (
                        "id",
                        EffectExpression::Input {
                            path: vec!["todoId".into()],
                        },
                    ),
                ],
                Some(EffectExpression::Input {
                    path: vec!["scope".into(), "tenantId".into()],
                }),
            ),
            confirmation_with_key(
                "project_first",
                [(
                    "id",
                    EffectExpression::Input {
                        path: vec!["todoId".into()],
                    },
                )],
                None,
            ),
        ];
        let canonical_wire = serde_json::json!({
            "scope": { "tenantId": "tenant-7" },
            "todoId": "todo-1"
        });

        let resolved = contract
            .resolve_projection_obligations(&canonical_wire)
            .unwrap();

        assert_eq!(
            resolved
                .iter()
                .map(|obligation| obligation.projector.as_str())
                .collect::<Vec<_>>(),
            ["project_second", "project_first"]
        );
        assert_eq!(
            resolved[0]
                .key
                .fields
                .iter()
                .map(|field| field.field.as_str())
                .collect::<Vec<_>>(),
            ["tenant_id", "id"]
        );
        assert_eq!(resolved[0].key.fields[0].value, "tenant-7");
        assert_eq!(resolved[0].key.fields[1].value, "todo-1");
        assert_eq!(resolved[0].partition, Some(serde_json::json!("tenant-7")));
        assert_eq!(resolved[1].key.fields[0].value, "todo-1");
        assert_eq!(resolved[1].partition, None);
    }

    #[test]
    fn projection_obligations_preserve_constants_and_nulls_through_serde() {
        let mut contract = typed_command::<Input, Accepted<Payload>>("todo.update").into_contract();
        let constant = serde_json::json!({
            "nested": [1, "two", null],
            "large": u64::MAX,
        });
        contract.confirmations = vec![confirmation_with_key(
            "project_todos",
            [(
                "constant_key",
                EffectExpression::Constant {
                    value: constant.clone(),
                },
            )],
            Some(EffectExpression::Null),
        )];

        let resolved = contract
            .resolve_projection_obligations(&serde_json::json!({}))
            .unwrap();

        assert_eq!(resolved[0].key.fields[0].value, constant);
        assert_eq!(resolved[0].partition, Some(serde_json::Value::Null));

        let encoded = serde_json::to_value(&resolved).unwrap();
        assert!(encoded[0]["partition"].is_null());
        let decoded: Vec<ResolvedProjectionObligation> = serde_json::from_value(encoded).unwrap();
        assert_eq!(decoded, resolved);
    }

    #[test]
    fn projection_obligation_resolution_fails_on_absent_input_paths() {
        let mut contract = typed_command::<Input, Accepted<Payload>>("todo.update").into_contract();
        contract.confirmations = vec![confirmation_with_key(
            "project_todos",
            [(
                "tenant_id",
                EffectExpression::Input {
                    path: vec!["scope".into(), "tenantId".into()],
                },
            )],
            None,
        )];

        let error = contract
            .resolve_projection_obligations(&serde_json::json!({ "scope": null }))
            .unwrap_err();

        assert!(matches!(
            error,
            ProjectionObligationResolutionError::MissingInputPath { path, .. }
                if path == ["scope", "tenantId"]
        ));
    }

    #[test]
    fn projection_obligation_resolution_rejects_unresolved_private_expressions() {
        let mut contract = typed_command::<Input, Accepted<Payload>>("todo.update").into_contract();
        contract.confirmations = vec![confirmation_with_key(
            "project_todos",
            [(
                "tenant_id",
                EffectExpression::TrustedPreset {
                    name: "tenant".into(),
                },
            )],
            None,
        )];
        assert!(matches!(
            contract.resolve_projection_obligations(&serde_json::json!({})),
            Err(ProjectionObligationResolutionError::TrustedPresetUnavailable {
                preset,
                ..
            }) if preset == "tenant"
        ));

        contract.confirmations = vec![confirmation_with_key(
            "project_todos",
            [],
            Some(EffectExpression::InvalidConstant {
                error: "not portable".into(),
            }),
        )];
        assert!(matches!(
            contract.resolve_projection_obligations(&serde_json::json!({})),
            Err(ProjectionObligationResolutionError::InvalidConstant {
                target,
                error,
                ..
            }) if target == "partition" && error == "not portable"
        ));
    }

    #[test]
    fn projection_obligation_resolution_is_empty_without_confirmations() {
        let contract = typed_command::<Input, Accepted<Payload>>("todo.check").into_contract();

        assert!(contract
            .resolve_projection_obligations(&serde_json::json!("unused"))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn finite_confirmation_requires_a_reachable_staged_outbox_fact() {
        let mut contract = typed_command::<Input, Accepted<Payload>>("todo.create").into_contract();
        contract.confirmations = vec![confirmation_with_facts(&["todo.created", "todo.recreated"])];
        let prepared = PreparedCommand::<Accepted<Payload>>::prepare(Payload {
            id: "todo-1".into(),
        })
        .unwrap();

        let no_fact = prepared
            .validate_commit_evidence(&contract, false, &[], &[])
            .unwrap_err();
        assert!(matches!(
            no_fact,
            CommandCommitProofError::UnreachableConfirmation { .. }
        ));

        let unrelated = OutboxMessage::create("message-1", "account.changed", Vec::new()).unwrap();
        assert!(matches!(
            prepared.validate_commit_evidence(&contract, false, &[unrelated], &[]),
            Err(CommandCommitProofError::UnreachableConfirmation { .. })
        ));

        let reachable = OutboxMessage::create("message-2", "todo.created", Vec::new()).unwrap();
        prepared
            .validate_commit_evidence(&contract, false, &[reachable], &[])
            .unwrap();

        let directed =
            OutboxMessage::create_to("message-3", "todo.created", "todo-projector", Vec::new())
                .unwrap();
        assert!(matches!(
            prepared.validate_commit_evidence(&contract, false, &[directed], &[]),
            Err(CommandCommitProofError::UnreachableConfirmation { .. })
        ));

        let mut published = OutboxMessage::create("message-4", "todo.created", Vec::new()).unwrap();
        published.status = crate::outbox::OutboxMessageStatus::Published;
        assert!(matches!(
            prepared.validate_commit_evidence(&contract, false, &[published], &[]),
            Err(CommandCommitProofError::UnreachableConfirmation { .. })
        ));

        let mut failed = OutboxMessage::create("message-5", "todo.created", Vec::new()).unwrap();
        failed.status = crate::outbox::OutboxMessageStatus::Failed;
        assert!(matches!(
            prepared.validate_commit_evidence(&contract, false, &[failed], &[]),
            Err(CommandCommitProofError::UnreachableConfirmation { .. })
        ));
    }

    #[test]
    fn accepted_without_confirmations_allows_an_empty_domain_batch() {
        let contract = typed_command::<Input, Accepted<Payload>>("todo.check").into_contract();
        let prepared = PreparedCommand::<Accepted<Payload>>::prepare(Payload {
            id: "todo-1".into(),
        })
        .unwrap();

        prepared
            .validate_commit_evidence(&contract, false, &[], &[])
            .unwrap();
    }

    #[test]
    fn fact_without_a_finite_confirmation_fails_at_commit_validation() {
        let contract = typed_command::<Input, Fact<Payload>>("todo.create").into_contract();
        let prepared = PreparedCommand::<Fact<Payload>>::prepare(Payload {
            id: "todo-1".into(),
        })
        .unwrap();
        let fact = OutboxMessage::create("message-1", "todo.created", Vec::new()).unwrap();

        assert_eq!(
            prepared
                .validate_commit_evidence(&contract, false, &[fact], &[])
                .unwrap_err(),
            CommandCommitProofError::FactHasNoConfirmations
        );
    }

    #[test]
    fn per_route_fingerprint_is_stable_and_contract_sensitive() {
        let first = typed_command::<Input, Accepted<Payload>>("todo.create")
            .roles(["writer", "admin"])
            .into_contract();
        let reordered = typed_command::<Input, Accepted<Payload>>("todo.create")
            .roles(["admin", "writer"])
            .into_contract();
        let renamed = typed_command::<Input, Accepted<Payload>>("todo.rename")
            .roles(["admin", "writer"])
            .into_contract();

        assert_eq!(first.fingerprint_bytes(), reordered.fingerprint_bytes());
        assert_ne!(first.fingerprint_bytes(), renamed.fingerprint_bytes());
    }

    #[test]
    fn binding_rejects_missing_graphql_type_ids() {
        let mut contract = typed_command::<Input, Accepted<Payload>>("todo.create").into_contract();
        contract.input.type_id = None;
        let error = TypedServiceCommandBinding::from_contracts("todos", &[contract]).unwrap_err();
        assert!(error.contains("input GraphQL metadata is missing"));
    }

    #[test]
    fn binding_canonicalizes_fields_and_roles_but_preserves_effect_order() {
        let mut first = typed_command::<Input, Accepted<Payload>>("todo.create")
            .roles(["writer", "admin"])
            .into_contract();
        first.input.fields.push(GraphqlTypeField {
            name: "z_extra".into(),
            type_name: "String".into(),
            nullable: true,
            list: false,
            item_nullable: false,
            nested: None,
        });
        first.effects.operations = vec![
            CommandEffect::InvalidateModel {
                model: "Zed".into(),
            },
            CommandEffect::InvalidateModel {
                model: "Alpha".into(),
            },
        ];

        let mut second = first.clone();
        second.roles.reverse();
        second.input.fields.reverse();
        let first = TypedServiceCommandBinding::from_contracts("todos", &[first]).unwrap();
        let second = TypedServiceCommandBinding::from_contracts("todos", &[second]).unwrap();
        assert_eq!(first, second);

        let mut reordered = typed_command::<Input, Accepted<Payload>>("todo.create")
            .roles(["writer", "admin"])
            .into_contract();
        reordered.input.fields.push(GraphqlTypeField {
            name: "z_extra".into(),
            type_name: "String".into(),
            nullable: true,
            list: false,
            item_nullable: false,
            nested: None,
        });
        reordered.effects.operations = vec![
            CommandEffect::InvalidateModel {
                model: "Alpha".into(),
            },
            CommandEffect::InvalidateModel {
                model: "Zed".into(),
            },
        ];
        let reordered = TypedServiceCommandBinding::from_contracts("todos", &[reordered]).unwrap();
        assert_ne!(
            first.structural_fingerprint,
            reordered.structural_fingerprint
        );
    }
}
