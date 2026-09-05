//! Same modeled execution proof across memory, SQLite and PostgreSQL.
use std::sync::Arc;

use crate::domain_event::DomainEventContract;
use crate::projection::executor::prepare_portable_projection;
use crate::projection::lower::{EventualOnly, ProjectionDescriptor};
use crate::projection_protocol::*;
use crate::{
    DomainEventDescriptor, DomainEventEnvelope, DomainEventOccurrence, RelationalReadModel, RowKey,
    RowValue,
};

#[derive(
    Clone, Debug, serde::Serialize, serde::Deserialize, crate::DomainState, crate::ReadModel,
)]
#[domain_state(version = 1)]
#[readmodel(table = "source_snapshot_rows", primary_key = ["id"])]
struct SnapshotRow {
    id: String,
    title: String,
}
type SourceSnapshotRows = SnapshotRow;

struct Changed;
impl DomainEventContract for Changed {
    const EVENT_NAME: &'static str = "snapshot.changed";
    const EVENT_VERSION: u64 = 1;
    fn descriptor() -> DomainEventDescriptor {
        DomainEventDescriptor::state::<SnapshotRow>("snapshot.changed", 1)
    }
}
struct Removed;
impl DomainEventContract for Removed {
    const EVENT_NAME: &'static str = "snapshot.removed";
    const EVENT_VERSION: u64 = 1;
    fn descriptor() -> DomainEventDescriptor {
        DomainEventDescriptor::state::<SnapshotRow>("snapshot.removed", 1)
    }
}
#[allow(non_snake_case)]
fn SaveSnapshot() -> crate::Mutation<()> {
    crate::mutation_file!("tests/fixtures/source_snapshot_save.graphql")
}
#[allow(non_snake_case)]
fn DeleteSnapshot() -> crate::Mutation<()> {
    crate::mutation_file!("tests/fixtures/source_snapshot_delete.graphql")
}
crate::projection! {
    const SNAPSHOTS: ProjectionDescriptor<EventualOnly> = {
        name: "source-snapshot-test",
        version: 1,
        epoch: "snapshot-v1",
        model: SnapshotRow,
        source: aggregate_snapshot,
        on { events: [Changed], mutation: SaveSnapshot, input: { row: body }, },
        on { events: [Removed], mutation: DeleteSnapshot, input: { id: aggregate_id }, },
    };
}

fn event(id: &str, sequence: u64, title: &str, delete: bool) -> DomainEventOccurrence {
    occurrence(id, sequence, 0, id, title, delete)
}

fn occurrence(
    id: &str,
    sequence: u64,
    ordinal: u32,
    row_id: &str,
    title: &str,
    delete: bool,
) -> DomainEventOccurrence {
    let mut occurrence = DomainEventOccurrence::capture(
        if delete {
            Removed::descriptor()
        } else {
            Changed::descriptor()
        },
        DomainEventEnvelope {
            aggregate_type: "snapshot-item".into(),
            aggregate_id: id.into(),
            aggregate_sequence: sequence,
            publication_ordinal: ordinal,
            occurred_at: std::time::UNIX_EPOCH,
            metadata: Default::default(),
        },
        &SnapshotRow {
            id: row_id.into(),
            title: title.into(),
        },
    )
    .unwrap();
    occurrence.overwrite_causation_id(&format!("cause-{id}-{sequence}"));
    occurrence
}

struct Harness {
    codec: Arc<ProjectionScopeCodec>,
}
impl Harness {
    async fn new(store: &impl ProjectionProtocolStore) -> Self {
        let topology = ProjectorTopologyId::new(1, "source-snapshot-test", [0x8e; 32]).unwrap();
        let codec = Arc::new(
            ProjectionScopeCodec::with_models(
                topology.clone(),
                [("SnapshotRow", SnapshotRow::schema())],
            )
            .unwrap(),
        );
        store
            .register_projection_models(
                &topology,
                &[ProjectionModelOwnership::new("SnapshotRow", "source_snapshot_rows").unwrap()],
            )
            .await
            .unwrap();
        Self { codec }
    }

    async fn prepare(
        &self,
        store: &impl ProjectionProtocolStore,
        occurrence: &DomainEventOccurrence,
        delivery: u64,
    ) -> Result<ProjectionCommitBatch, ProjectionProtocolError> {
        let partition = self.codec.encode_partition(None).unwrap();
        let input = TrustedProjectionInput::mint(
            ProjectionInputCursor::new(
                self.codec.topology().clone(),
                partition,
                ProjectionSource::new("test-broker", b"stream".to_vec()).unwrap(),
                ProjectionEpoch::new("broker-v1").unwrap(),
                delivery,
            )
            .unwrap(),
            ProjectionInputFingerprint::from_canonical_bytes(
                &occurrence.canonical_bytes().unwrap(),
            ),
            occurrence.id(),
            occurrence.causation_id().unwrap(),
            ProjectionGeneration::initial(),
            false,
        )
        .unwrap();
        let mut workspace = ProjectionWorkspace::new(
            self.codec.clone(),
            None,
            input,
            ProjectionEpoch::new("snapshot-v1").unwrap(),
        )
        .unwrap();
        let lowered = SNAPSHOTS
            .server_executor()
            .unwrap()
            .plan(occurrence)
            .unwrap();
        let prepared = prepare_portable_projection(&workspace, lowered)?;
        let snapshots = store
            .projection_execution_snapshot_batch(prepared.snapshot_request())
            .await?;
        prepared.stage(&mut workspace, snapshots)?;
        workspace.into_batch()
    }

    async fn apply(
        &self,
        store: &impl ProjectionProtocolStore,
        occurrence: &DomainEventOccurrence,
        delivery: u64,
    ) -> Result<ProjectionCommitResult, ProjectionProtocolError> {
        store
            .commit_projection(self.prepare(store, occurrence, delivery).await?)
            .await
    }

    async fn read(
        &self,
        store: &impl ProjectionProtocolStore,
        id: &str,
    ) -> ProjectionQuerySnapshot {
        store
            .projection_query_snapshot(
                &ProjectionQuerySnapshotRequest::new(
                    &self.codec,
                    None,
                    "SnapshotRow",
                    RowKey::new([("id", RowValue::String(id.into()))]),
                    Vec::new(),
                )
                .unwrap(),
            )
            .await
            .unwrap()
    }
}

async fn matrix(store: &impl ProjectionProtocolStore) {
    let h = Harness::new(store).await;
    let first = h
        .apply(store, &event("a", 3, "new", false), 1)
        .await
        .unwrap();
    assert_eq!(first.records.len(), 1);
    for (delivery, sequence) in [(2, 2), (3, 1)] {
        let stale = h
            .apply(store, &event("a", sequence, "old", false), delivery)
            .await
            .unwrap();
        assert!(
            stale.records.is_empty(),
            "stale source must not allocate a row revision"
        );
        let observations: Vec<_> = stale
            .changes
            .iter()
            .filter(|change| change.kind == ProjectionChangeKind::Observation)
            .collect();
        assert_eq!(
            observations.len(),
            1,
            "stale causation must observe current state"
        );
        assert_eq!(
            observations[0].revision,
            Some(first.records[0].revision.clone())
        );
    }
    let snapshot = h.read(store, "a").await;
    assert_eq!(
        snapshot.row.unwrap().get_serde::<String>("title").unwrap(),
        "new"
    );
    assert_eq!(snapshot.record.unwrap().revision, first.records[0].revision);

    // Deletion before every earlier write must still establish a durable fence.
    h.apply(store, &event("b", 5, "", true), 4).await.unwrap();
    h.apply(store, &event("b", 1, "late create", false), 5)
        .await
        .unwrap();
    let deleted = h.read(store, "b").await;
    assert!(deleted.row.is_none());
    assert!(deleted.record.unwrap().tombstone);
    // A newer snapshot explicitly recreates the row; stale deletion cannot undo it.
    h.apply(store, &event("b", 6, "recreated", false), 6)
        .await
        .unwrap();
    h.apply(store, &event("b", 4, "", true), 7).await.unwrap();
    let recreated = h.read(store, "b").await;
    assert_eq!(
        recreated.row.unwrap().get_serde::<String>("title").unwrap(),
        "recreated"
    );
    assert_eq!(recreated.record.unwrap().revision.incarnation(), 2);
    // Repeated newer deletions advance the fence even while the row is absent.
    h.apply(store, &event("b", 7, "", true), 8).await.unwrap();
    h.apply(store, &event("b", 9, "", true), 9).await.unwrap();
    h.apply(store, &event("b", 8, "delayed recreate", false), 10)
        .await
        .unwrap();
    assert!(h.read(store, "b").await.row.is_none());

    // Occurrence IDs alone do not authenticate the body at an equal version.
    assert!(h
        .prepare(store, &event("a", 3, "conflict", false), 11)
        .await
        .is_err());
    // A different stream cannot overwrite another aggregate's row.
    assert!(h
        .prepare(
            store,
            &occurrence("intruder", 99, 0, "a", "takeover", false),
            11
        )
        .await
        .is_err());
    // Different aggregates with equal sequence have independent clocks.
    h.apply(store, &event("c", 1, "independent", false), 11)
        .await
        .unwrap();
    assert!(h.read(store, "c").await.row.is_some());

    // Exact occurrence replay is a transport duplicate, not a second mutation.
    let e = event("a", 4, "latest", false);
    h.apply(store, &e, 12).await.unwrap();
    let replay = h.apply(store, &e, 12).await.unwrap();
    assert_eq!(replay.outcome, ProjectionCommitOutcome::Duplicate);
    assert!(replay.records.is_empty());

    // A stale-read observer cannot falsely confirm a row changed by a concurrent commit.
    h.apply(store, &event("race", 3, "initial", false), 13)
        .await
        .unwrap();
    let stale = h
        .prepare(store, &event("race", 2, "old", false), 15)
        .await
        .unwrap();
    h.apply(store, &event("race", 5, "racing", false), 14)
        .await
        .unwrap();
    assert!(matches!(
        store.commit_projection(stale).await,
        Err(ProjectionProtocolError::RecordRevisionConflict { .. })
    ));

    // Multiple publications in one aggregate commit share a causation but have
    // an ordered publication ordinal. Late confirmation is still idempotent.
    h.apply(
        store,
        &occurrence("ordinal", 1, 1, "ordinal", "second", false),
        16,
    )
    .await
    .unwrap();
    h.apply(
        store,
        &occurrence("ordinal", 1, 0, "ordinal", "first", false),
        17,
    )
    .await
    .unwrap();
    assert_eq!(
        h.read(store, "ordinal")
            .await
            .row
            .unwrap()
            .get_serde::<String>("title")
            .unwrap(),
        "second"
    );

    // Even a lower-level writer cannot clear an existing source fence.
    let mut unfenced = h
        .prepare(store, &event("a", 6, "unsafe", false), 19)
        .await
        .unwrap();
    unfenced.mutations[0].source_snapshot = None;
    assert!(matches!(
        store.commit_projection(unfenced).await,
        Err(ProjectionProtocolError::InvalidBatch(_))
    ));
    assert_eq!(
        h.read(store, "a")
            .await
            .row
            .unwrap()
            .get_serde::<String>("title")
            .unwrap(),
        "latest"
    );

    // Migration does not invent a source version for old read-model rows.
    let mut unversioned = h
        .prepare(store, &event("unversioned", 1, "old schema", false), 19)
        .await
        .unwrap();
    unversioned.mutations[0].source_snapshot = None;
    store.commit_projection(unversioned).await.unwrap();
    assert!(h
        .prepare(store, &event("unversioned", 2, "new schema", false), 20)
        .await
        .is_err());
}

#[tokio::test]
async fn source_snapshots_memory_reordering_and_atomic_confirmation() {
    matrix(&crate::InMemoryRepository::new()).await;
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn source_snapshots_sqlite_reordering_and_restart() {
    use crate::table::TableSchemaRegistry;
    let path = std::env::temp_dir().join(format!(
        "distributed-source-snapshots-{}.sqlite",
        uuid::Uuid::now_v7()
    ));
    let url = format!("sqlite:{}?mode=rwc", path.display());
    let store = crate::SqliteRepository::connect_and_migrate(&url)
        .await
        .unwrap();
    let mut registry = TableSchemaRegistry::new();
    registry
        .register_schema(SnapshotRow::schema().clone())
        .unwrap();
    store
        .bootstrap_table_schema_for_dev(&registry)
        .await
        .unwrap();
    matrix(&store).await;
    store.pool().close().await;
    let restarted = crate::SqliteRepository::connect_and_migrate(&url)
        .await
        .unwrap();
    let h = Harness::new(&restarted).await;
    h.apply(&restarted, &event("b", 2, "after restart", false), 21)
        .await
        .unwrap();
    assert!(h.read(&restarted, "b").await.row.is_none());
    restarted.pool().close().await;
    std::fs::remove_file(path).unwrap();
}

#[cfg(feature = "postgres")]
#[tokio::test]
async fn source_snapshots_postgres_reordering_and_restart() {
    let Ok(url) = std::env::var("DISTRIBUTED_SNAPSHOT_TEST_POSTGRES_URL") else {
        return;
    };
    let store = crate::PostgresRepository::connect_and_migrate(&url)
        .await
        .unwrap();
    let mut registry = crate::table::TableSchemaRegistry::new();
    registry
        .register_schema(SnapshotRow::schema().clone())
        .unwrap();
    store
        .bootstrap_table_schema_for_dev(&registry)
        .await
        .unwrap();
    matrix(&store).await;
    store.pool().close().await;
    let restarted = crate::PostgresRepository::connect_and_migrate(&url)
        .await
        .unwrap();
    let h = Harness::new(&restarted).await;
    h.apply(&restarted, &event("b", 2, "after restart", false), 21)
        .await
        .unwrap();
    assert!(h.read(&restarted, "b").await.row.is_none());
}

#[test]
fn source_snapshots_policy_is_explicit_hashed_and_rejects_delta_programs() {
    use crate::projection::{
        ProjectionArm, ProjectionMutationKind as Kind, ProjectionOperation, ProjectionPartition,
        ProjectionProgram,
    };
    let snapshots = SNAPSHOTS.program().unwrap();
    let ordered = ProjectionProgram::try_new(
        snapshots.name(),
        snapshots.version(),
        ProjectionPartition::Unit,
        snapshots.arms().to_vec(),
    )
    .unwrap();
    assert!(snapshots.source_snapshots());
    assert!(!ordered.source_snapshots());
    assert_ne!(snapshots.id().unwrap(), ordered.id().unwrap());
    let arm = &snapshots.arms()[0];
    let op = &arm.operations()[0];
    for kind in [Kind::Insert, Kind::Patch, Kind::UpsertPatch, Kind::Recreate] {
        let operation = ProjectionOperation::try_new(
            op.operation_id(),
            0,
            kind,
            op.target().clone(),
            op.key().to_vec(),
            op.fields().to_vec(),
            vec![],
            vec![],
        )
        .unwrap();
        let arm =
            ProjectionArm::try_new(arm.arm_id(), arm.selector().clone(), vec![operation]).unwrap();
        let program =
            ProjectionProgram::try_new("delta", 1, ProjectionPartition::Unit, vec![arm]).unwrap();
        assert!(
            program.with_source_snapshots().is_err(),
            "{kind:?} cannot silently become a snapshot"
        );
    }
    let partitioned = ProjectionProgram::try_new(
        "partitioned",
        1,
        ProjectionPartition::Expression(op.key()[0].expression().clone()),
        snapshots.arms().to_vec(),
    )
    .unwrap();
    assert!(partitioned.with_source_snapshots().is_err());
}

#[test]
fn source_snapshots_reject_direct_materialization() {
    use crate::projection::placement::*;
    let result = ProjectionBinding::materialize_direct(
        DirectProjectionPlacement::new(&SNAPSHOTS),
        ProjectionSourceBinding::try_new("snapshot-domain", "ordered-domain-events", 1).unwrap(),
        ProjectionOwner::try_new("snapshot-direct").unwrap(),
        "distributed-projection-partition",
        1,
        vec![ProjectionOutput::try_new(
            "SnapshotRow",
            "source_snapshot_rows",
            SnapshotRow::schema().clone(),
        )
        .unwrap()],
        vec![],
        None,
    );
    assert!(matches!(
        result,
        Err(ProjectionTopologyError::DirectIneligible { .. })
    ));
}
