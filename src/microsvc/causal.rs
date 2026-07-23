//! Framework-owned staging for typed causal command handlers.
//!
//! A workspace deliberately exposes neither its repository nor a commit
//! operation. Handlers receive owned aggregate checkouts and may only return
//! them to this staging area. The service dispatcher validates the resulting
//! evidence, wraps its ordinary [`CommitBatch`] in the command-ledger fence,
//! and is the only code allowed to commit it.

#![cfg_attr(not(feature = "graphql"), allow(dead_code))]

use std::collections::HashSet;
use std::future::Future;
use std::ops::{Deref, DerefMut};
use std::pin::Pin;
use std::sync::Mutex;

use serde::Serialize;

use crate::aggregate::{hydrate, Aggregate, AggregateRepository};
use crate::command_ledger::CausalGetStream;
use crate::graphql::command_contract::{
    CommandCommitProofError, CommandOutcome, ProjectionCommitProof, TypedCommandContract,
};
use crate::graphql::{GraphqlOutputType, PrepareCommandError, PreparedCommand, Projected};
use crate::outbox::OutboxMessage;
use crate::read_model::{ReadModelWritePlanBuilder, RelationalReadModel};
use crate::repository::{CommitBatch, RepositoryError, SnapshotWrite, StreamIdentity, StreamWrite};
use crate::table::{TableStoreError, TableWritePlan};

type LoadAggregateFuture<'a, A> =
    Pin<Box<dyn Future<Output = Result<Option<A>, RepositoryError>> + Send + 'a>>;

/// Erases the concrete backend from the handler-facing workspace while
/// retaining the repository's explicitly non-locking causal load capability.
trait CausalAggregateStore<A>: Send + Sync {
    fn load<'a>(&'a self, identity: &'a StreamIdentity) -> LoadAggregateFuture<'a, A>;

    fn snapshot_writes(
        &self,
        aggregate: &A,
    ) -> Result<(Vec<SnapshotWrite>, Option<u64>), RepositoryError>;
}

struct AggregateStoreRef<'repo, R, A> {
    repository: &'repo AggregateRepository<R, A>,
}

impl<R, A> CausalAggregateStore<A> for AggregateStoreRef<'_, R, A>
where
    R: CausalGetStream,
    A: Aggregate + Send + Sync + 'static,
{
    fn load<'a>(&'a self, identity: &'a StreamIdentity) -> LoadAggregateFuture<'a, A> {
        Box::pin(async move {
            let Some(entity) = self.repository.repo().get_causal_stream(identity).await? else {
                return Ok(None);
            };
            hydrate::<A>(entity).map(Some)
        })
    }

    fn snapshot_writes(
        &self,
        aggregate: &A,
    ) -> Result<(Vec<SnapshotWrite>, Option<u64>), RepositoryError> {
        self.repository.snapshot_writes_for(aggregate)
    }
}

#[derive(Clone, Debug)]
enum CheckoutOrigin {
    New,
    Loaded {
        identity: StreamIdentity,
        committed_version: u64,
    },
}

/// An aggregate value checked out of a causal workspace.
///
/// The value is owned, so no repository or queue lock survives across handler
/// awaits. Its original stream identity and committed version remain private
/// and are checked when the handler stages it back into the workspace.
pub struct AggregateCheckout<A: Aggregate> {
    aggregate: A,
    origin: CheckoutOrigin,
}

impl<A: Aggregate> AggregateCheckout<A> {
    fn loaded(identity: StreamIdentity, aggregate: A) -> Self {
        let committed_version = aggregate.entity().committed_version();
        Self {
            aggregate,
            origin: CheckoutOrigin::Loaded {
                identity,
                committed_version,
            },
        }
    }

    fn new(aggregate: A) -> Self {
        Self {
            aggregate,
            origin: CheckoutOrigin::New,
        }
    }

    pub fn aggregate(&self) -> &A {
        &self.aggregate
    }

    pub fn aggregate_mut(&mut self) -> &mut A {
        &mut self.aggregate
    }
}

impl<A: Aggregate> Deref for AggregateCheckout<A> {
    type Target = A;

    fn deref(&self) -> &Self::Target {
        &self.aggregate
    }
}

impl<A: Aggregate> DerefMut for AggregateCheckout<A> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.aggregate
    }
}

/// A deterministic staging error caught before command commit I/O.
#[derive(Debug)]
pub(crate) enum CausalWorkspaceError {
    Repository(RepositoryError),
    Table(TableStoreError),
    Prepare(PrepareCommandError),
    Poisoned,
    IdentityChanged {
        original: StreamIdentity,
        current: StreamIdentity,
    },
    CommittedVersionChanged {
        identity: StreamIdentity,
        original: u64,
        current: u64,
    },
    DuplicateStream(StreamIdentity),
    ProjectionAlreadyStaged,
    CommitBatchAlreadyPrepared,
}

impl std::fmt::Display for CausalWorkspaceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Repository(error) => write!(formatter, "causal aggregate load failed: {error}"),
            Self::Table(error) => write!(formatter, "causal read-model staging failed: {error}"),
            Self::Prepare(error) => write!(formatter, "causal result preparation failed: {error}"),
            Self::Poisoned => formatter.write_str("causal workspace lock poisoned"),
            Self::IdentityChanged { original, current } => write!(
                formatter,
                "checked-out aggregate identity changed from `{original}` to `{current}`"
            ),
            Self::CommittedVersionChanged {
                identity,
                original,
                current,
            } => write!(
                formatter,
                "checked-out aggregate `{identity}` changed committed version from {original} to {current}"
            ),
            Self::DuplicateStream(identity) => {
                write!(formatter, "aggregate stream `{identity}` was staged more than once")
            }
            Self::ProjectionAlreadyStaged => formatter.write_str(
                "a causal command may prepare only one same-transaction projected result",
            ),
            Self::CommitBatchAlreadyPrepared => {
                formatter.write_str("causal workspace commit batch was already prepared")
            }
        }
    }
}

impl std::error::Error for CausalWorkspaceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Repository(error) => Some(error),
            Self::Table(error) => Some(error),
            Self::Prepare(error) => Some(error),
            Self::Poisoned
            | Self::IdentityChanged { .. }
            | Self::CommittedVersionChanged { .. }
            | Self::DuplicateStream(_)
            | Self::ProjectionAlreadyStaged
            | Self::CommitBatchAlreadyPrepared => None,
        }
    }
}

impl From<RepositoryError> for CausalWorkspaceError {
    fn from(error: RepositoryError) -> Self {
        Self::Repository(error)
    }
}

impl From<TableStoreError> for CausalWorkspaceError {
    fn from(error: TableStoreError) -> Self {
        Self::Table(error)
    }
}

impl From<PrepareCommandError> for CausalWorkspaceError {
    fn from(error: PrepareCommandError) -> Self {
        Self::Prepare(error)
    }
}

struct StagedAggregate<A> {
    identity: StreamIdentity,
    aggregate: A,
    snapshot_version: Option<u64>,
}

struct WorkspaceState<A> {
    aggregates: Vec<StagedAggregate<A>>,
    identities: HashSet<StreamIdentity>,
    outbox_messages: Vec<OutboxMessage>,
    read_model_plans: Vec<TableWritePlan>,
    snapshots: Vec<SnapshotWrite>,
    projection_staged: bool,
}

impl<A> Default for WorkspaceState<A> {
    fn default() -> Self {
        Self {
            aggregates: Vec::new(),
            identities: HashSet::new(),
            outbox_messages: Vec::new(),
            read_model_plans: Vec::new(),
            snapshots: Vec::new(),
            projection_staged: false,
        }
    }
}

/// A handler-scoped unit of work. The concrete repository is erased and never
/// exposed through this type.
pub(crate) struct CausalWorkspace<'repo, A>
where
    A: Aggregate + Send + Sync + 'static,
{
    store: Box<dyn CausalAggregateStore<A> + 'repo>,
    state: Mutex<WorkspaceState<A>>,
}

impl<'repo, A> CausalWorkspace<'repo, A>
where
    A: Aggregate + Send + Sync + 'static,
{
    pub(crate) fn new<R>(repository: &'repo AggregateRepository<R, A>) -> Self
    where
        R: CausalGetStream + 'repo,
    {
        Self {
            store: Box::new(AggregateStoreRef { repository }),
            state: Mutex::new(WorkspaceState::default()),
        }
    }

    /// Load a full stream through the explicit causal/non-locking capability.
    pub(crate) async fn load(
        &self,
        id: &str,
    ) -> Result<Option<AggregateCheckout<A>>, CausalWorkspaceError> {
        let identity = StreamIdentity::new(A::aggregate_type(), id)?;
        Ok(self
            .store
            .load(&identity)
            .await?
            .map(|aggregate| AggregateCheckout::loaded(identity, aggregate)))
    }

    /// Create an empty owned checkout. The aggregate must establish a valid ID
    /// before it is staged.
    pub(crate) fn create(&self) -> AggregateCheckout<A> {
        AggregateCheckout::new(A::new_empty())
    }

    /// Return a checkout to the unit of work after validating the original
    /// identity/version fence and duplicate stream membership.
    pub(crate) fn stage(&self, checkout: AggregateCheckout<A>) -> Result<(), CausalWorkspaceError> {
        let AggregateCheckout { aggregate, origin } = checkout;
        let identity = StreamIdentity::new(A::aggregate_type(), aggregate.entity().id())?;
        let committed_version = aggregate.entity().committed_version();

        match origin {
            CheckoutOrigin::New => {
                if committed_version != 0 {
                    return Err(CausalWorkspaceError::CommittedVersionChanged {
                        identity,
                        original: 0,
                        current: committed_version,
                    });
                }
            }
            CheckoutOrigin::Loaded {
                identity: original_identity,
                committed_version: original_version,
            } => {
                if identity != original_identity {
                    return Err(CausalWorkspaceError::IdentityChanged {
                        original: original_identity,
                        current: identity,
                    });
                }
                if committed_version != original_version {
                    return Err(CausalWorkspaceError::CommittedVersionChanged {
                        identity,
                        original: original_version,
                        current: committed_version,
                    });
                }
            }
        }

        let (mut snapshots, snapshot_version) = self.store.snapshot_writes(&aggregate)?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| CausalWorkspaceError::Poisoned)?;
        if !state.identities.insert(identity.clone()) {
            return Err(CausalWorkspaceError::DuplicateStream(identity));
        }
        state.snapshots.append(&mut snapshots);
        state.aggregates.push(StagedAggregate {
            identity,
            aggregate,
            snapshot_version,
        });
        Ok(())
    }

    pub(crate) fn stage_outbox(&self, message: OutboxMessage) -> Result<(), CausalWorkspaceError> {
        self.state
            .lock()
            .map_err(|_| CausalWorkspaceError::Poisoned)?
            .outbox_messages
            .push(message);
        Ok(())
    }

    pub(crate) fn stage_read_models(
        &self,
        builder: ReadModelWritePlanBuilder,
    ) -> Result<(), CausalWorkspaceError> {
        let plan = builder.into_write_plan()?;
        self.stage_read_model_plan(plan)
    }

    pub(crate) fn stage_read_model_plan(
        &self,
        plan: TableWritePlan,
    ) -> Result<(), CausalWorkspaceError> {
        plan.validate()?;
        self.state
            .lock()
            .map_err(|_| CausalWorkspaceError::Poisoned)?
            .read_model_plans
            .push(plan);
        Ok(())
    }

    /// Atomically stage the returned view as one full-row upsert and prepare a
    /// non-forgeable `Projected<M>` completion tied to that exact row.
    pub(crate) fn prepare_projected<M>(
        &self,
        model: M,
    ) -> Result<PreparedCommand<Projected<M>>, CausalWorkspaceError>
    where
        M: GraphqlOutputType + RelationalReadModel + Serialize + Send + Sync + 'static,
    {
        let mut builder = ReadModelWritePlanBuilder::new();
        builder.upsert(&model)?;
        let plan = builder.into_write_plan()?;
        let proof = ProjectionCommitProof::for_model(&model)?;
        let prepared = PreparedCommand::prepare_projected(model, proof)?;

        let mut state = self
            .state
            .lock()
            .map_err(|_| CausalWorkspaceError::Poisoned)?;
        if state.projection_staged {
            return Err(CausalWorkspaceError::ProjectionAlreadyStaged);
        }
        state.projection_staged = true;
        state.read_model_plans.push(plan);
        Ok(prepared)
    }

    pub(crate) fn into_parts(self) -> Result<CausalWorkspaceParts<A>, CausalWorkspaceError> {
        let state = self
            .state
            .into_inner()
            .map_err(|_| CausalWorkspaceError::Poisoned)?;
        Ok(CausalWorkspaceParts {
            aggregates: state.aggregates,
            outbox_messages: state.outbox_messages,
            read_model_plans: state.read_model_plans,
            snapshots: state.snapshots,
            batch_prepared: false,
        })
    }
}

/// Owned staged work retained by the dispatcher between handler completion and
/// construction of the ledger-fenced commit batch.
pub(crate) struct CausalWorkspaceParts<A> {
    aggregates: Vec<StagedAggregate<A>>,
    outbox_messages: Vec<OutboxMessage>,
    read_model_plans: Vec<TableWritePlan>,
    snapshots: Vec<SnapshotWrite>,
    batch_prepared: bool,
}

impl<A> CausalWorkspaceParts<A>
where
    A: Aggregate,
{
    pub(crate) fn validate_prepared<K: CommandOutcome>(
        &self,
        contract: &TypedCommandContract,
        prepared: &PreparedCommand<K>,
    ) -> Result<(), CommandCommitProofError> {
        prepared.validate_commit_evidence(
            contract,
            self.aggregates
                .iter()
                .any(|staged| !staged.aggregate.entity().new_events().is_empty()),
            &self.outbox_messages,
            &self.read_model_plans,
        )
    }

    /// Borrow staged aggregates into the existing public commit-batch shape.
    /// This is one-shot because its owned participants move into the batch.
    pub(crate) fn prepare_commit_batch(&mut self) -> Result<CommitBatch<'_>, CausalWorkspaceError> {
        if self.batch_prepared {
            return Err(CausalWorkspaceError::CommitBatchAlreadyPrepared);
        }
        self.batch_prepared = true;

        // Preserve ordinary AggregateCommit source metadata when the causal
        // unit of work has one unambiguous source aggregate. A multi-aggregate
        // command must associate each message explicitly (for example through
        // OutboxMessage::domain_event); guessing would create false ordering
        // evidence for downstream projectors.
        if let [staged] = self.aggregates.as_slice() {
            for message in &mut self.outbox_messages {
                message.set_source(&staged.aggregate);
            }
        }

        let streams = self
            .aggregates
            .iter_mut()
            .map(|staged| StreamWrite::new(staged.identity.clone(), staged.aggregate.entity_mut()))
            .collect();
        let mut batch = CommitBatch::new(streams);
        batch.outbox_messages = std::mem::take(&mut self.outbox_messages);
        batch.read_model_plans = std::mem::take(&mut self.read_model_plans);
        batch.snapshots = std::mem::take(&mut self.snapshots);
        Ok(batch)
    }

    /// Apply snapshot cache bookkeeping only after the fenced commit succeeds.
    pub(crate) fn mark_snapshot_versions_committed(&mut self) {
        for staged in &mut self.aggregates {
            if let Some(version) = staged.snapshot_version {
                staged.aggregate.entity_mut().set_snapshot_version(version);
            }
        }
    }

    #[cfg(test)]
    fn aggregate_count(&self) -> usize {
        self.aggregates.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use crate::entity::{Entity, EventRecord};
    use crate::graphql::{GraphqlTypeDef, GraphqlTypeField};
    use crate::table::{
        ColumnType, PrimaryKey, RowKey, RowValue, RowValues, TableColumn, TableKind, TableMutation,
        TableSchema,
    };

    #[derive(Clone, Default)]
    struct TestAggregate {
        entity: Entity,
    }

    impl Aggregate for TestAggregate {
        type ReplayError = String;

        fn aggregate_type() -> &'static str {
            "workspace_test"
        }

        fn entity(&self) -> &Entity {
            &self.entity
        }

        fn entity_mut(&mut self) -> &mut Entity {
            &mut self.entity
        }

        fn replay_event(&mut self, _event: &EventRecord) -> Result<(), Self::ReplayError> {
            Ok(())
        }
    }

    #[derive(Clone)]
    struct TestRepo {
        entity: Arc<Entity>,
    }

    impl CausalGetStream for TestRepo {
        fn get_causal_stream<'a>(
            &'a self,
            identity: &'a StreamIdentity,
        ) -> impl Future<Output = Result<Option<Entity>, RepositoryError>> + Send + 'a {
            async move {
                Ok((identity.aggregate_id() == self.entity.id()).then(|| (*self.entity).clone()))
            }
        }
    }

    #[derive(Clone, Debug, Serialize)]
    struct TestView {
        id: String,
        title: String,
    }

    impl RelationalReadModel for TestView {
        fn schema() -> &'static TableSchema {
            static SCHEMA: std::sync::LazyLock<TableSchema> =
                std::sync::LazyLock::new(|| TableSchema {
                    model_name: "TestView".into(),
                    table_name: "test_views".into(),
                    columns: vec![
                        TableColumn {
                            primary_key: true,
                            ..TableColumn::new("id", "id", ColumnType::Text)
                        },
                        TableColumn::new("title", "title", ColumnType::Text),
                    ],
                    primary_key: PrimaryKey::new(["id"]),
                    version_column: None,
                    foreign_keys: Vec::new(),
                    indexes: Vec::new(),
                    relationships: Vec::new(),
                    kind: TableKind::ReadModel,
                });
            &SCHEMA
        }

        fn primary_key(&self) -> Result<RowKey, TableStoreError> {
            Ok(RowKey::new([(
                "id",
                crate::table::RowValue::String(self.id.clone()),
            )]))
        }

        fn to_row(&self) -> Result<RowValues, TableStoreError> {
            let mut values = RowValues::new();
            values.insert("id", crate::table::RowValue::String(self.id.clone()));
            values.insert("title", crate::table::RowValue::String(self.title.clone()));
            Ok(values)
        }

        fn from_row(row: RowValues) -> Result<Self, TableStoreError> {
            Ok(Self {
                id: row.get_serde("id")?,
                title: row.get_serde("title")?,
            })
        }
    }

    impl GraphqlOutputType for TestView {
        fn graphql_type() -> GraphqlTypeDef {
            GraphqlTypeDef::new(
                "TestView",
                vec![
                    GraphqlTypeField {
                        name: "id".into(),
                        type_name: "String".into(),
                        nullable: false,
                        list: false,
                        item_nullable: false,
                        nested: None,
                    },
                    GraphqlTypeField {
                        name: "title".into(),
                        type_name: "String".into(),
                        nullable: false,
                        list: false,
                        item_nullable: false,
                        nested: None,
                    },
                ],
            )
            .with_type_id(std::any::TypeId::of::<Self>())
        }
    }

    #[derive(serde::Deserialize)]
    struct TestInput {}

    impl crate::graphql::GraphqlInputType for TestInput {
        fn graphql_type() -> GraphqlTypeDef {
            GraphqlTypeDef::new("TestInput", Vec::new())
                .with_type_id(std::any::TypeId::of::<Self>())
        }
    }

    fn loaded_repo() -> AggregateRepository<TestRepo, TestAggregate> {
        let mut entity = Entity::with_id("a-1");
        entity
            .digest_empty("Created")
            .expect("test event should encode");
        entity.mark_committed();
        AggregateRepository::new(TestRepo {
            entity: Arc::new(entity),
        })
    }

    #[tokio::test]
    async fn load_is_owned_and_stage_rejects_identity_and_version_tampering() {
        let repository = loaded_repo();
        let workspace = CausalWorkspace::new(&repository);

        let mut renamed = workspace.load("a-1").await.unwrap().unwrap();
        renamed.entity_mut().set_id("other");
        assert!(matches!(
            workspace.stage(renamed),
            Err(CausalWorkspaceError::IdentityChanged { .. })
        ));

        let mut version_changed = workspace.load("a-1").await.unwrap().unwrap();
        version_changed.entity_mut().load_from_history(Vec::new());
        assert!(matches!(
            workspace.stage(version_changed),
            Err(CausalWorkspaceError::CommittedVersionChanged { .. })
        ));
    }

    #[tokio::test]
    async fn duplicate_streams_are_rejected_without_repository_locks() {
        let repository = loaded_repo();
        let workspace = CausalWorkspace::new(&repository);
        let first = workspace.load("a-1").await.unwrap().unwrap();
        let second = workspace.load("a-1").await.unwrap().unwrap();
        workspace.stage(first).unwrap();
        assert!(matches!(
            workspace.stage(second),
            Err(CausalWorkspaceError::DuplicateStream(_))
        ));
    }

    #[tokio::test]
    async fn create_stages_a_new_owned_aggregate_after_it_establishes_identity() {
        let repository = loaded_repo();
        let workspace = CausalWorkspace::new(&repository);
        let mut checkout = workspace.create();
        checkout.entity_mut().set_id("new-aggregate");
        checkout.entity_mut().digest_empty("Created").unwrap();

        workspace.stage(checkout).unwrap();

        let parts = workspace.into_parts().unwrap();
        assert_eq!(parts.aggregate_count(), 1);
    }

    #[tokio::test]
    async fn workspace_drains_all_commit_participants_once() {
        let repository = loaded_repo();
        let workspace = CausalWorkspace::new(&repository);
        let mut checkout = workspace.load("a-1").await.unwrap().unwrap();
        checkout.entity_mut().digest_empty("Changed").unwrap();
        workspace.stage(checkout).unwrap();
        workspace
            .stage_outbox(OutboxMessage::create("m-1", "test.changed", vec![1]).unwrap())
            .unwrap();
        let mut plan = ReadModelWritePlanBuilder::new();
        plan.upsert(&TestView {
            id: "v-1".into(),
            title: "first".into(),
        })
        .unwrap();
        workspace.stage_read_models(plan).unwrap();

        let mut parts = workspace.into_parts().unwrap();
        assert_eq!(parts.aggregate_count(), 1);
        {
            let batch = parts.prepare_commit_batch().unwrap();
            assert_eq!(batch.streams.len(), 1);
            assert_eq!(batch.outbox_messages.len(), 1);
            assert_eq!(batch.read_model_plans.len(), 1);
            assert_eq!(
                batch.outbox_messages[0].source_aggregate_type.as_deref(),
                Some(TestAggregate::aggregate_type())
            );
            assert_eq!(
                batch.outbox_messages[0].source_aggregate_id.as_deref(),
                Some("a-1")
            );
            assert_eq!(batch.outbox_messages[0].source_sequence, Some(2));
        }
        assert!(matches!(
            parts.prepare_commit_batch(),
            Err(CausalWorkspaceError::CommitBatchAlreadyPrepared)
        ));
    }

    #[tokio::test]
    async fn projected_result_is_tied_to_one_exact_staged_upsert() {
        let repository = loaded_repo();
        let workspace = CausalWorkspace::new(&repository);
        let view = TestView {
            id: "v-1".into(),
            title: "projected".into(),
        };
        let prepared = workspace.prepare_projected(view).unwrap();
        let mut aggregate = workspace.load("a-1").await.unwrap().unwrap();
        aggregate.entity_mut().digest_empty("Projected").unwrap();
        workspace.stage(aggregate).unwrap();
        let parts = workspace.into_parts().unwrap();

        let contract =
            crate::graphql::typed_command::<TestInput, Projected<TestView>>("test.project")
                .into_contract();
        parts.validate_prepared(&contract, &prepared).unwrap();
    }

    #[tokio::test]
    async fn projected_result_requires_a_durable_domain_fact() {
        let repository = loaded_repo();
        let workspace = CausalWorkspace::new(&repository);
        let prepared = workspace
            .prepare_projected(TestView {
                id: "v-1".into(),
                title: "projected".into(),
            })
            .unwrap();
        let parts = workspace.into_parts().unwrap();
        let contract =
            crate::graphql::typed_command::<TestInput, Projected<TestView>>("test.project")
                .into_contract();

        assert!(matches!(
            parts.validate_prepared(&contract, &prepared),
            Err(CommandCommitProofError::DurableFactMissing)
        ));
    }

    #[tokio::test]
    async fn projected_proof_rejects_a_second_write_to_the_returned_key() {
        let repository = loaded_repo();
        let workspace = CausalWorkspace::new(&repository);
        let prepared = workspace
            .prepare_projected(TestView {
                id: "v-1".into(),
                title: "returned".into(),
            })
            .unwrap();
        workspace
            .stage_outbox(OutboxMessage::create("projection-2", "test.changed", vec![]).unwrap())
            .unwrap();
        let mut conflicting = ReadModelWritePlanBuilder::new();
        conflicting
            .upsert(&TestView {
                id: "v-1".into(),
                title: "overwritten".into(),
            })
            .unwrap();
        workspace.stage_read_models(conflicting).unwrap();
        let parts = workspace.into_parts().unwrap();
        let contract =
            crate::graphql::typed_command::<TestInput, Projected<TestView>>("test.project")
                .into_contract();

        assert!(matches!(
            parts.validate_prepared(&contract, &prepared),
            Err(CommandCommitProofError::ProjectionWriteConflict { .. })
        ));
    }

    #[tokio::test]
    async fn projected_proof_rejects_row_drift_after_preparation() {
        let repository = loaded_repo();
        let workspace = CausalWorkspace::new(&repository);
        let prepared = workspace
            .prepare_projected(TestView {
                id: "v-1".into(),
                title: "returned".into(),
            })
            .unwrap();
        workspace
            .stage_outbox(OutboxMessage::create("projection-3", "test.changed", vec![]).unwrap())
            .unwrap();
        let mut parts = workspace.into_parts().unwrap();
        let TableMutation::UpsertRow(mutation) = &mut parts.read_model_plans[0].mutations[0] else {
            panic!("projected preparation must stage a full-row upsert");
        };
        mutation
            .values
            .insert("title", RowValue::String("different".into()));
        let contract =
            crate::graphql::typed_command::<TestInput, Projected<TestView>>("test.project")
                .into_contract();

        assert!(matches!(
            parts.validate_prepared(&contract, &prepared),
            Err(CommandCommitProofError::ProjectionWriteMismatch { .. })
        ));
    }
}
