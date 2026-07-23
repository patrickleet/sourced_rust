//! Generic SQLx persistence for the durable projection protocol.
//!
//! SQLite and PostgreSQL share the same state machine and SQL shape. Every
//! mutating protocol operation first takes a portable partition lock by
//! performing a no-op upsert on `projection_partitions`; all semantic
//! decisions then happen before statements that could raise a constraint
//! error, because PostgreSQL aborts a transaction after such an error.

#![expect(
    clippy::manual_async_fn,
    reason = "trait impls return impl Future + Send to preserve Send bounds"
)]

use std::collections::{BTreeSet, HashMap};
use std::future::Future;
use std::pin::Pin;

use sqlx::{Encode, Executor, IntoArguments, Pool, QueryBuilder, Row, Transaction, Type};

use crate::projection_protocol::{
    ProjectionChange, ProjectionChangeCursor, ProjectionChangeKind, ProjectionChangeRead,
    ProjectionChangeRetention, ProjectionCheckpoint, ProjectionCommitBatch,
    ProjectionCommitOutcome, ProjectionCommitResult, ProjectionEpoch, ProjectionFailure,
    ProjectionFailureBatch, ProjectionFailureLocation, ProjectionGeneration, ProjectionInputCursor,
    ProjectionInputDisposition, ProjectionInputFingerprint, ProjectionLiveRecordBatch,
    ProjectionLiveRecordBatchRequest, ProjectionModelOwnership, ProjectionMutationKind,
    ProjectionObligationEvidence, ProjectionObligationEvidenceBatch,
    ProjectionObligationEvidenceBatchRequest, ProjectionObservation, ProjectionObservationKind,
    ProjectionObservationTarget, ProjectionPartition, ProjectionPartitionRuntimeState,
    ProjectionPendingRetry, ProjectionProtocolError, ProjectionProtocolStore,
    ProjectionQuerySnapshot, ProjectionQuerySnapshotBatch, ProjectionQuerySnapshotBatchRequest,
    ProjectionQuerySnapshotRequest, ProjectionRecordExpectation, ProjectionRecordMetadata,
    ProjectionRecordScope, ProjectionSource, ProjectorTopologyId, RecordRevision,
    SameTransactionProjectionBatch, SameTransactionProjectionEvidence, TrustedProjectionInput,
    MAX_PROJECTION_POSITION,
};
use crate::repository::RepositoryError;
use crate::sqlx_repo::read_model::{
    apply_read_model_write_plan_in_tx, push_key_predicates, quote_identifier, row_version_in_tx,
    validate_sql_write_plan, validate_values_match_key, version_column,
};
use crate::sqlx_repo::repo::{repository_storage_error, SqlxRepoBackend, SqlxRepository};
use crate::table::{
    validate_row_values, RowValues, TableMutation, TableStoreError, TableWritePlan,
};

pub(crate) const PROJECTION_CHANGE_NOTIFY_TABLE: &str = "projection_changes";

/// Reject ordinary/raw table writes once a model-wide causal owner exists.
///
/// Both this path and `register_projection_models` first acquire the same
/// durable per-table ownership fence in sorted order. The marker check and all
/// physical writes remain inside that transaction, so an absent marker cannot
/// race a first legacy row into a newly causal-owned table.
pub(crate) async fn reject_causal_table_writes_in_tx<DB>(
    tx: &mut Transaction<'_, DB>,
    tables: &BTreeSet<String>,
) -> Result<(), TableStoreError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
{
    lock_projection_table_ownership_fences_in_tx(tx, tables).await?;
    for table in tables {
        let mut builder = QueryBuilder::<DB>::new(
            "SELECT table_name FROM projection_causal_tables WHERE table_name = ",
        );
        builder.push_bind(table.as_str());
        builder.push(" LIMIT 1");
        let row = builder
            .build()
            .fetch_optional(&mut **tx)
            .await
            .map_err(|error| {
                crate::sqlx_repo::read_model_storage_error(
                    DB::BACKEND,
                    "check causal projection ownership",
                    error,
                )
            })?;
        if row.is_some() {
            return Err(TableStoreError::CausalWriteRequired {
                table: table.clone(),
            });
        }
    }
    Ok(())
}

async fn lock_projection_table_ownership_fences_in_tx<DB>(
    tx: &mut Transaction<'_, DB>,
    tables: &BTreeSet<String>,
) -> Result<(), TableStoreError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
{
    // BTreeSet iteration is deterministic, preventing multi-table writers and
    // registration batches from deadlocking on opposite lock orders.
    for table in tables {
        let mut insert = QueryBuilder::<DB>::new(
            "INSERT INTO projection_table_ownership_fences (table_name) VALUES (",
        );
        insert.push_bind(table.as_str());
        insert.push(") ON CONFLICT (table_name) DO NOTHING");
        insert.build().execute(&mut **tx).await.map_err(|error| {
            crate::sqlx_repo::read_model_storage_error(
                DB::BACKEND,
                "acquire causal projection ownership fence",
                error,
            )
        })?;

        let mut lock = QueryBuilder::<DB>::new(
            "SELECT table_name FROM projection_table_ownership_fences WHERE table_name = ",
        );
        lock.push_bind(table.as_str());
        if DB::BACKEND == "postgres" {
            lock.push(" FOR UPDATE");
        }
        if lock
            .build()
            .fetch_optional(&mut **tx)
            .await
            .map_err(|error| {
                crate::sqlx_repo::read_model_storage_error(
                    DB::BACKEND,
                    "lock causal projection ownership fence",
                    error,
                )
            })?
            .is_none()
        {
            return Err(TableStoreError::Storage(format!(
                "{} causal projection ownership fence `{table}` disappeared",
                DB::BACKEND
            )));
        }
    }
    Ok(())
}

#[derive(Clone, Debug)]
struct PartitionState {
    active_generation: ProjectionGeneration,
    change_epoch: ProjectionEpoch,
    change_head: u64,
    compacted_through: u64,
    pending_retry_failure_id: Option<String>,
    stopped_failure_id: Option<String>,
}

#[derive(Clone, Debug)]
struct StoredCursor {
    source_epoch: ProjectionEpoch,
    source_position: u64,
    input_fingerprint: ProjectionInputFingerprint,
    message_id: String,
    causation_id: String,
    gap_free: bool,
    change: ProjectionChangeCursor,
}

#[derive(Clone, Debug)]
struct StoredReceipt {
    source_bytes: Vec<u8>,
    source_hash: Vec<u8>,
    source_partition_bytes: Vec<u8>,
    source_partition_hash: Vec<u8>,
    source_epoch: ProjectionEpoch,
    source_position: u64,
    input_fingerprint: ProjectionInputFingerprint,
    message_id: String,
    causation_id: String,
    gap_free: bool,
    outcome_kind: String,
    change: ProjectionChangeCursor,
}

#[derive(Clone, Debug)]
struct StoredInputIdentity {
    partition_bytes: Vec<u8>,
    partition_hash: Vec<u8>,
    source_bytes: Vec<u8>,
    source_hash: Vec<u8>,
    source_partition_bytes: Vec<u8>,
    source_partition_hash: Vec<u8>,
    source_epoch: ProjectionEpoch,
    source_position: u64,
    input_fingerprint: ProjectionInputFingerprint,
    message_id: String,
    causation_id: String,
    gap_free: bool,
}

#[derive(Clone, Debug)]
struct StoredRecord {
    metadata: ProjectionRecordMetadata,
}

struct StoredFailure {
    failure: ProjectionFailure,
}

enum InputDisposition {
    New,
    Duplicate(ProjectionCheckpoint),
    Stale(ProjectionCheckpoint),
}

fn protocol_storage_error<DB: SqlxRepoBackend>(
    operation: &str,
    error: sqlx::Error,
) -> ProjectionProtocolError {
    ProjectionProtocolError::Repository(repository_storage_error::<DB>(operation, error))
}

fn corrupt_storage(message: impl Into<String>) -> ProjectionProtocolError {
    ProjectionProtocolError::InvalidBatch(format!(
        "corrupt projection protocol storage: {}",
        message.into()
    ))
}

fn to_i64<DB: SqlxRepoBackend>(
    value: u64,
    field: &'static str,
) -> Result<i64, ProjectionProtocolError> {
    i64::try_from(value).map_err(|_| {
        ProjectionProtocolError::Repository(RepositoryError::Model(format!(
            "{} {field} value {value} exceeds signed bigint storage",
            DB::BACKEND
        )))
    })
}

fn from_i64<DB: SqlxRepoBackend>(
    value: i64,
    field: &'static str,
) -> Result<u64, ProjectionProtocolError> {
    u64::try_from(value)
        .map_err(|_| corrupt_storage(format!("{} {field} value {value} is negative", DB::BACKEND)))
}

fn digest_bytes(value: [u8; 32]) -> Vec<u8> {
    value.to_vec()
}

fn decode_digest(value: Vec<u8>, field: &'static str) -> Result<[u8; 32], ProjectionProtocolError> {
    value.try_into().map_err(|value: Vec<u8>| {
        corrupt_storage(format!(
            "{field} digest contains {} bytes instead of 32",
            value.len()
        ))
    })
}

fn verify_bytes(
    actual: &[u8],
    expected: &[u8],
    field: &'static str,
) -> Result<(), ProjectionProtocolError> {
    if actual == expected {
        Ok(())
    } else {
        Err(corrupt_storage(format!(
            "{field} bytes do not match their hash lookup"
        )))
    }
}

fn verify_digest(
    actual: &[u8],
    expected: [u8; 32],
    field: &'static str,
) -> Result<(), ProjectionProtocolError> {
    verify_bytes(actual, &expected, field)
}

fn checked_next(value: u64, domain: &'static str) -> Result<u64, ProjectionProtocolError> {
    if value >= MAX_PROJECTION_POSITION {
        return Err(ProjectionProtocolError::PositionOverflow { domain });
    }
    Ok(value + 1)
}

fn table_model_name(mutation: &TableMutation) -> &str {
    match mutation {
        TableMutation::UpsertRow(mutation) => &mutation.schema.model_name,
        TableMutation::PatchRow(mutation) => &mutation.schema.model_name,
        TableMutation::DeleteRow(mutation) => &mutation.schema.model_name,
    }
}

async fn physical_row_exists_in_tx<DB>(
    tx: &mut Transaction<'_, DB>,
    mutation: &TableMutation,
) -> Result<bool, ProjectionProtocolError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    let (schema, key) = match mutation {
        TableMutation::UpsertRow(mutation) => (mutation.schema, &mutation.key),
        TableMutation::PatchRow(mutation) => (mutation.schema, &mutation.key),
        TableMutation::DeleteRow(mutation) => (mutation.schema, &mutation.key),
    };
    Ok(row_version_in_tx(tx, schema, key).await?.is_some())
}

fn change_kind_for_mutation(kind: ProjectionMutationKind) -> ProjectionChangeKind {
    match kind {
        ProjectionMutationKind::Upsert => ProjectionChangeKind::RecordUpsert,
        ProjectionMutationKind::Delete => ProjectionChangeKind::RecordDelete,
        ProjectionMutationKind::Recreate => ProjectionChangeKind::RecordRecreate,
    }
}

fn decode_change_kind(value: &str) -> Result<ProjectionChangeKind, ProjectionProtocolError> {
    match value {
        "checkpoint" => Ok(ProjectionChangeKind::Checkpoint),
        "record_upsert" => Ok(ProjectionChangeKind::RecordUpsert),
        "record_delete" => Ok(ProjectionChangeKind::RecordDelete),
        "record_recreate" => Ok(ProjectionChangeKind::RecordRecreate),
        "observation" => Ok(ProjectionChangeKind::Observation),
        "failure" => Ok(ProjectionChangeKind::Failure),
        other => Err(corrupt_storage(format!(
            "unknown projection change kind `{other}`"
        ))),
    }
}

fn decode_observation_kind(
    value: &str,
) -> Result<ProjectionObservationKind, ProjectionProtocolError> {
    match value {
        "record" => Ok(ProjectionObservationKind::Record),
        "dependency" => Ok(ProjectionObservationKind::Dependency),
        other => Err(corrupt_storage(format!(
            "unknown projection observation kind `{other}`"
        ))),
    }
}

fn decode_partition_row<DB>(
    row: &DB::Row,
    topology: &ProjectorTopologyId,
    partition: &crate::projection_protocol::ProjectionPartition,
) -> Result<PartitionState, ProjectionProtocolError>
where
    DB: SqlxRepoBackend,
    for<'q> i64: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> String: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> Vec<u8>: Type<DB> + sqlx::Decode<'q, DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    let topology_bytes: Vec<u8> = row
        .try_get("topology_bytes")
        .map_err(|error| protocol_storage_error::<DB>("decode projection topology bytes", error))?;
    let partition_bytes: Vec<u8> = row.try_get("partition_bytes").map_err(|error| {
        protocol_storage_error::<DB>("decode projection partition bytes", error)
    })?;
    verify_bytes(
        &topology_bytes,
        &topology.canonical_bytes(),
        "projector topology",
    )?;
    verify_bytes(
        &partition_bytes,
        partition.canonical_bytes(),
        "projection partition",
    )?;

    let active_generation = ProjectionGeneration::new(from_i64::<DB>(
        row.try_get("active_generation").map_err(|error| {
            protocol_storage_error::<DB>("decode projection active generation", error)
        })?,
        "projection active generation",
    )?)?;
    let change_epoch: String = row
        .try_get("change_epoch")
        .map_err(|error| protocol_storage_error::<DB>("decode projection change epoch", error))?;
    let change_epoch = ProjectionEpoch::new(change_epoch)?;
    let change_head = from_i64::<DB>(
        row.try_get("change_head").map_err(|error| {
            protocol_storage_error::<DB>("decode projection change head", error)
        })?,
        "projection change head",
    )?;
    let compacted_through = from_i64::<DB>(
        row.try_get("compacted_through").map_err(|error| {
            protocol_storage_error::<DB>("decode projection compaction watermark", error)
        })?,
        "projection compaction watermark",
    )?;
    if compacted_through > change_head {
        return Err(corrupt_storage(
            "projection compaction watermark exceeds change head",
        ));
    }
    let stopped_failure_id = row.try_get("stopped_failure_id").map_err(|error| {
        protocol_storage_error::<DB>("decode stopped projection failure", error)
    })?;
    let pending_retry_failure_id = row
        .try_get("pending_retry_failure_id")
        .map_err(|error| protocol_storage_error::<DB>("decode pending projection retry", error))?;
    Ok(PartitionState {
        active_generation,
        change_epoch,
        change_head,
        compacted_through,
        pending_retry_failure_id,
        stopped_failure_id,
    })
}

async fn lock_partition_in_tx<DB>(
    tx: &mut Transaction<'_, DB>,
    topology: &ProjectorTopologyId,
    partition: &crate::projection_protocol::ProjectionPartition,
    change_epoch: &ProjectionEpoch,
) -> Result<PartitionState, ProjectionProtocolError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> String: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> Vec<u8>: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
    for<'q> &'q [u8]: Encode<'q, DB> + Type<DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    let topology_hash = topology.digest();
    let partition_hash = partition.digest();
    let topology_bytes = topology.canonical_bytes();
    let mut builder = QueryBuilder::<DB>::new(
        "INSERT INTO projection_partitions \
         (topology_bytes, topology_hash, partition_bytes, partition_hash, active_generation, change_epoch) \
         VALUES (",
    );
    builder.push_bind(topology_bytes.as_slice());
    builder.push(", ");
    builder.push_bind(topology_hash.as_slice());
    builder.push(", ");
    builder.push_bind(partition.canonical_bytes());
    builder.push(", ");
    builder.push_bind(partition_hash.as_slice());
    builder.push(", 1, ");
    builder.push_bind(change_epoch.as_str());
    builder.push(
        ") ON CONFLICT (topology_hash, partition_hash) DO UPDATE \
         SET topology_hash = excluded.topology_hash \
         RETURNING topology_bytes, partition_bytes, active_generation, change_epoch, \
         change_head, compacted_through, pending_retry_failure_id, stopped_failure_id",
    );
    let row = builder
        .build()
        .fetch_one(&mut **tx)
        .await
        .map_err(|error| protocol_storage_error::<DB>("lock projection partition", error))?;
    let state = decode_partition_row::<DB>(&row, topology, partition)?;
    if state.change_epoch != *change_epoch {
        return Err(ProjectionProtocolError::IncomparableInput);
    }

    let mut generation = QueryBuilder::<DB>::new(
        "INSERT INTO projection_generations \
         (topology_hash, partition_hash, generation, retry_of_generation, retry_of_failure_id) \
         VALUES (",
    );
    generation.push_bind(topology_hash.as_slice());
    generation.push(", ");
    generation.push_bind(partition_hash.as_slice());
    generation.push(
        ", 1, NULL, NULL) ON CONFLICT (topology_hash, partition_hash, generation) DO NOTHING",
    );
    generation
        .build()
        .execute(&mut **tx)
        .await
        .map_err(|error| {
            protocol_storage_error::<DB>("ensure initial projection generation", error)
        })?;
    verify_generation_exists_in_tx::<DB>(tx, topology, partition, state.active_generation).await?;
    Ok(state)
}

async fn lock_existing_partition_in_tx<DB>(
    tx: &mut Transaction<'_, DB>,
    topology: &ProjectorTopologyId,
    partition: &crate::projection_protocol::ProjectionPartition,
) -> Result<Option<PartitionState>, ProjectionProtocolError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> String: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> Vec<u8>: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> &'q [u8]: Encode<'q, DB> + Type<DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    let topology_hash = topology.digest();
    let partition_hash = partition.digest();
    let mut builder = QueryBuilder::<DB>::new(
        "UPDATE projection_partitions SET topology_hash = topology_hash WHERE topology_hash = ",
    );
    builder.push_bind(topology_hash.as_slice());
    builder.push(" AND partition_hash = ");
    builder.push_bind(partition_hash.as_slice());
    builder.push(
        " RETURNING topology_bytes, partition_bytes, active_generation, change_epoch, \
         change_head, compacted_through, pending_retry_failure_id, stopped_failure_id",
    );
    let row = builder
        .build()
        .fetch_optional(&mut **tx)
        .await
        .map_err(|error| protocol_storage_error::<DB>("lock projection partition", error))?;
    row.map(|row| decode_partition_row::<DB>(&row, topology, partition))
        .transpose()
}

async fn load_partition<DB>(
    pool: &Pool<DB>,
    topology: &ProjectorTopologyId,
    partition: &crate::projection_protocol::ProjectionPartition,
) -> Result<Option<PartitionState>, ProjectionProtocolError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c Pool<DB>: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> String: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> Vec<u8>: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> &'q [u8]: Encode<'q, DB> + Type<DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    let topology_hash = topology.digest();
    let partition_hash = partition.digest();
    let mut builder = QueryBuilder::<DB>::new(
        "SELECT topology_bytes, partition_bytes, active_generation, change_epoch, \
         change_head, compacted_through, pending_retry_failure_id, stopped_failure_id \
         FROM projection_partitions WHERE topology_hash = ",
    );
    builder.push_bind(topology_hash.as_slice());
    builder.push(" AND partition_hash = ");
    builder.push_bind(partition_hash.as_slice());
    let row = builder
        .build()
        .fetch_optional(pool)
        .await
        .map_err(|error| protocol_storage_error::<DB>("load projection partition", error))?;
    row.map(|row| decode_partition_row::<DB>(&row, topology, partition))
        .transpose()
}

async fn load_partition_in_tx<DB>(
    tx: &mut Transaction<'_, DB>,
    topology: &ProjectorTopologyId,
    partition: &ProjectionPartition,
) -> Result<Option<PartitionState>, ProjectionProtocolError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> String: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> Vec<u8>: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> &'q [u8]: Encode<'q, DB> + Type<DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    load_partition_in_connection(&mut **tx, topology, partition).await
}

async fn load_partition_in_connection<DB>(
    connection: &mut DB::Connection,
    topology: &ProjectorTopologyId,
    partition: &ProjectionPartition,
) -> Result<Option<PartitionState>, ProjectionProtocolError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> String: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> Vec<u8>: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> &'q [u8]: Encode<'q, DB> + Type<DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    let topology_hash = topology.digest();
    let partition_hash = partition.digest();
    let mut builder = QueryBuilder::<DB>::new(
        "SELECT topology_bytes, partition_bytes, active_generation, change_epoch, \
         change_head, compacted_through, pending_retry_failure_id, stopped_failure_id \
         FROM projection_partitions WHERE topology_hash = ",
    );
    builder.push_bind(topology_hash.as_slice());
    builder.push(" AND partition_hash = ");
    builder.push_bind(partition_hash.as_slice());
    let row = builder
        .build()
        .fetch_optional(&mut *connection)
        .await
        .map_err(|error| protocol_storage_error::<DB>("load projection partition", error))?;
    row.map(|row| decode_partition_row::<DB>(&row, topology, partition))
        .transpose()
}

/// Load the runtime fence and its immutable pending-retry identity from one
/// database statement. PostgreSQL's default READ COMMITTED isolation gives
/// each statement its own snapshot, so independent partition/failure selects
/// could otherwise report a mixed repair state.
async fn load_partition_runtime_state<DB>(
    pool: &Pool<DB>,
    topology: &ProjectorTopologyId,
    partition: &ProjectionPartition,
) -> Result<Option<ProjectionPartitionRuntimeState>, ProjectionProtocolError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c Pool<DB>: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> String: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> Vec<u8>: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> &'q [u8]: Encode<'q, DB> + Type<DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    let topology_hash = topology.digest();
    let partition_hash = partition.digest();
    let mut builder = QueryBuilder::<DB>::new(
        "SELECT partition.topology_bytes, partition.partition_bytes, \
         partition.active_generation, partition.change_epoch, partition.change_head, \
         partition.compacted_through, partition.pending_retry_failure_id, \
         partition.stopped_failure_id, failure.failure_id AS retry_failure_id, \
         failure.source_bytes AS retry_source_bytes, failure.source_hash AS retry_source_hash, \
         failure.source_partition_bytes AS retry_source_partition_bytes, \
         failure.source_partition_hash AS retry_source_partition_hash, \
         failure.source_epoch AS retry_source_epoch, \
         failure.source_position AS retry_source_position, \
         failure.input_hash AS retry_input_hash, failure.message_id AS retry_message_id, \
         failure.causation_id AS retry_causation_id, failure.gap_free AS retry_gap_free, \
         failure.generation AS retry_failed_generation \
         FROM projection_partitions partition LEFT JOIN projection_failures failure \
         ON failure.topology_hash = partition.topology_hash \
         AND failure.partition_hash = partition.partition_hash \
         AND failure.failure_id = partition.pending_retry_failure_id \
         WHERE partition.topology_hash = ",
    );
    builder.push_bind(topology_hash.as_slice());
    builder.push(" AND partition.partition_hash = ");
    builder.push_bind(partition_hash.as_slice());
    let Some(row) = builder
        .build()
        .fetch_optional(pool)
        .await
        .map_err(|error| {
            protocol_storage_error::<DB>("load projection partition runtime state", error)
        })?
    else {
        return Ok(None);
    };

    let state = decode_partition_row::<DB>(&row, topology, partition)?;
    let pending_retry = match &state.pending_retry_failure_id {
        Some(expected_failure_id) => {
            if state.stopped_failure_id.is_some() {
                return Err(corrupt_storage(
                    "projection partition is both stopped and pending retry",
                ));
            }
            let failure_id = row
                .try_get::<Option<String>, _>("retry_failure_id")
                .map_err(|error| {
                    protocol_storage_error::<DB>("decode pending retry failure ID", error)
                })?
                .ok_or_else(|| {
                    corrupt_storage(format!(
                        "pending projection retry failure `{expected_failure_id}` is missing"
                    ))
                })?;
            if &failure_id != expected_failure_id {
                return Err(corrupt_storage(
                    "pending retry join returned the wrong failure",
                ));
            }
            let source_bytes = row
                .try_get::<Option<Vec<u8>>, _>("retry_source_bytes")
                .map_err(|error| {
                    protocol_storage_error::<DB>("decode pending retry source bytes", error)
                })?
                .ok_or_else(|| corrupt_storage("pending retry source bytes are missing"))?;
            let source_hash = row
                .try_get::<Option<Vec<u8>>, _>("retry_source_hash")
                .map_err(|error| {
                    protocol_storage_error::<DB>("decode pending retry source hash", error)
                })?
                .ok_or_else(|| corrupt_storage("pending retry source hash is missing"))?;
            let source_partition_bytes = row
                .try_get::<Option<Vec<u8>>, _>("retry_source_partition_bytes")
                .map_err(|error| {
                    protocol_storage_error::<DB>(
                        "decode pending retry source partition bytes",
                        error,
                    )
                })?
                .ok_or_else(|| {
                    corrupt_storage("pending retry source partition bytes are missing")
                })?;
            let source_partition_hash = row
                .try_get::<Option<Vec<u8>>, _>("retry_source_partition_hash")
                .map_err(|error| {
                    protocol_storage_error::<DB>(
                        "decode pending retry source partition hash",
                        error,
                    )
                })?
                .ok_or_else(|| corrupt_storage("pending retry source partition hash is missing"))?;
            let source =
                ProjectionSource::from_canonical_name_bytes(&source_bytes, source_partition_bytes)?;
            verify_digest(
                &source_hash,
                source.digest(),
                "pending retry projection source",
            )?;
            verify_digest(
                &source_partition_hash,
                source.partition_digest(),
                "pending retry projection source partition",
            )?;
            let source_epoch = row
                .try_get::<Option<String>, _>("retry_source_epoch")
                .map_err(|error| {
                    protocol_storage_error::<DB>("decode pending retry source epoch", error)
                })?
                .ok_or_else(|| corrupt_storage("pending retry source epoch is missing"))?;
            let source_position = from_i64::<DB>(
                row.try_get::<Option<i64>, _>("retry_source_position")
                    .map_err(|error| {
                        protocol_storage_error::<DB>("decode pending retry source position", error)
                    })?
                    .ok_or_else(|| corrupt_storage("pending retry source position is missing"))?,
                "pending retry source position",
            )?;
            let input_fingerprint = ProjectionInputFingerprint::from_digest(decode_digest(
                row.try_get::<Option<Vec<u8>>, _>("retry_input_hash")
                    .map_err(|error| {
                        protocol_storage_error::<DB>("decode pending retry input hash", error)
                    })?
                    .ok_or_else(|| corrupt_storage("pending retry input hash is missing"))?,
                "pending retry input",
            )?);
            let message_id = row
                .try_get::<Option<String>, _>("retry_message_id")
                .map_err(|error| {
                    protocol_storage_error::<DB>("decode pending retry message ID", error)
                })?
                .ok_or_else(|| corrupt_storage("pending retry message ID is missing"))?;
            let causation_id = row
                .try_get::<Option<String>, _>("retry_causation_id")
                .map_err(|error| {
                    protocol_storage_error::<DB>("decode pending retry causation ID", error)
                })?
                .ok_or_else(|| corrupt_storage("pending retry causation ID is missing"))?;
            let gap_free = match row
                .try_get::<Option<i64>, _>("retry_gap_free")
                .map_err(|error| {
                    protocol_storage_error::<DB>("decode pending retry gap-free flag", error)
                })?
                .ok_or_else(|| corrupt_storage("pending retry gap-free flag is missing"))?
            {
                0 => false,
                1 => true,
                value => {
                    return Err(corrupt_storage(format!(
                        "pending retry gap-free flag contains invalid value {value}"
                    )))
                }
            };
            let failed_generation = ProjectionGeneration::new(from_i64::<DB>(
                row.try_get::<Option<i64>, _>("retry_failed_generation")
                    .map_err(|error| {
                        protocol_storage_error::<DB>(
                            "decode pending retry failed generation",
                            error,
                        )
                    })?
                    .ok_or_else(|| corrupt_storage("pending retry failed generation is missing"))?,
                "pending retry failed generation",
            )?)?;
            if failed_generation.checked_next()? != state.active_generation {
                return Err(corrupt_storage(format!(
                    "pending retry failure generation {} does not precede active generation {}",
                    failed_generation.get(),
                    state.active_generation.get()
                )));
            }
            Some(ProjectionPendingRetry {
                failure_id,
                input: ProjectionInputCursor::new(
                    topology.clone(),
                    partition.clone(),
                    source,
                    ProjectionEpoch::new(source_epoch)?,
                    source_position,
                )?,
                input_fingerprint,
                message_id,
                causation_id,
                failed_generation,
                gap_free,
            })
        }
        None => None,
    };

    Ok(Some(ProjectionPartitionRuntimeState {
        active_generation: state.active_generation,
        stopped_failure_id: state.stopped_failure_id,
        pending_retry,
    }))
}

async fn verify_generation_exists_in_tx<DB>(
    tx: &mut Transaction<'_, DB>,
    topology: &ProjectorTopologyId,
    partition: &crate::projection_protocol::ProjectionPartition,
    generation: ProjectionGeneration,
) -> Result<(), ProjectionProtocolError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
    for<'q> &'q [u8]: Encode<'q, DB> + Type<DB>,
{
    let generation_value = to_i64::<DB>(generation.get(), "projection generation")?;
    let topology_hash = topology.digest();
    let partition_hash = partition.digest();
    let mut builder =
        QueryBuilder::<DB>::new("SELECT 1 FROM projection_generations WHERE topology_hash = ");
    builder.push_bind(topology_hash.as_slice());
    builder.push(" AND partition_hash = ");
    builder.push_bind(partition_hash.as_slice());
    builder.push(" AND generation = ");
    builder.push_bind(generation_value);
    builder.push(" LIMIT 1");
    if builder
        .build()
        .fetch_optional(&mut **tx)
        .await
        .map_err(|error| protocol_storage_error::<DB>("verify projection generation", error))?
        .is_none()
    {
        return Err(corrupt_storage(format!(
            "active projection generation {} is missing",
            generation.get()
        )));
    }
    Ok(())
}

fn ensure_active_input(
    state: &PartitionState,
    input: &TrustedProjectionInput,
) -> Result<(), ProjectionProtocolError> {
    if state.active_generation != input.generation {
        return Err(ProjectionProtocolError::GenerationFenced {
            expected: state.active_generation.get(),
            actual: input.generation.get(),
        });
    }
    if let Some(failure_id) = &state.stopped_failure_id {
        return Err(ProjectionProtocolError::PartitionStopped {
            failure_id: failure_id.clone(),
        });
    }
    Ok(())
}

fn verify_stored_change(
    state: &PartitionState,
    change: &ProjectionChangeCursor,
) -> Result<(), ProjectionProtocolError> {
    if change.epoch() != &state.change_epoch || change.position() > state.change_head {
        return Err(corrupt_storage(
            "stored projection outcome change is outside its partition head",
        ));
    }
    Ok(())
}

fn checkpoint_from_stored(
    cursor_scope: &ProjectionInputCursor,
    source_epoch: ProjectionEpoch,
    source_position: u64,
    change: ProjectionChangeCursor,
    gap_free: bool,
) -> Result<ProjectionCheckpoint, ProjectionProtocolError> {
    ProjectionCheckpoint::new(
        ProjectionInputCursor::new(
            cursor_scope.topology().clone(),
            cursor_scope.projection_partition().clone(),
            cursor_scope.source().clone(),
            source_epoch,
            source_position,
        )?,
        change,
        gap_free,
    )
    .map_err(ProjectionProtocolError::from)
}

fn decode_stored_receipt<DB>(
    row: &DB::Row,
    topology: &ProjectorTopologyId,
    partition: &ProjectionPartition,
) -> Result<StoredReceipt, ProjectionProtocolError>
where
    DB: SqlxRepoBackend,
    for<'q> i64: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> String: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> Vec<u8>: Type<DB> + sqlx::Decode<'q, DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    let source_bytes: Vec<u8> = row
        .try_get("source_bytes")
        .map_err(|error| protocol_storage_error::<DB>("decode receipt source bytes", error))?;
    let source_hash: Vec<u8> = row
        .try_get("source_hash")
        .map_err(|error| protocol_storage_error::<DB>("decode receipt source hash", error))?;
    let source_partition_bytes: Vec<u8> =
        row.try_get("source_partition_bytes").map_err(|error| {
            protocol_storage_error::<DB>("decode receipt source partition bytes", error)
        })?;
    let source_partition_hash: Vec<u8> = row.try_get("source_partition_hash").map_err(|error| {
        protocol_storage_error::<DB>("decode receipt source partition hash", error)
    })?;
    let source =
        ProjectionSource::from_canonical_name_bytes(&source_bytes, source_partition_bytes.clone())?;
    verify_digest(&source_hash, source.digest(), "projection receipt source")?;
    verify_digest(
        &source_partition_hash,
        source.partition_digest(),
        "projection receipt source partition",
    )?;
    let source_epoch: String = row
        .try_get("source_epoch")
        .map_err(|error| protocol_storage_error::<DB>("decode receipt source epoch", error))?;
    let source_epoch = ProjectionEpoch::new(source_epoch)?;
    let source_position = from_i64::<DB>(
        row.try_get("source_position").map_err(|error| {
            protocol_storage_error::<DB>("decode receipt source position", error)
        })?,
        "receipt source position",
    )?;
    let input_hash = decode_digest(
        row.try_get("input_hash")
            .map_err(|error| protocol_storage_error::<DB>("decode receipt input hash", error))?,
        "projection input",
    )?;
    let message_id = row
        .try_get("message_id")
        .map_err(|error| protocol_storage_error::<DB>("decode receipt message ID", error))?;
    let causation_id = row
        .try_get("causation_id")
        .map_err(|error| protocol_storage_error::<DB>("decode receipt causation ID", error))?;
    let gap_free = match row
        .try_get::<i64, _>("gap_free")
        .map_err(|error| protocol_storage_error::<DB>("decode receipt gap-free flag", error))?
    {
        0 => false,
        1 => true,
        value => {
            return Err(corrupt_storage(format!(
                "receipt gap-free flag contains invalid value {value}"
            )))
        }
    };
    let outcome_kind: String = row
        .try_get("outcome_kind")
        .map_err(|error| protocol_storage_error::<DB>("decode receipt outcome", error))?;
    if outcome_kind != "applied" && outcome_kind != "failed" {
        return Err(corrupt_storage(format!(
            "unknown projection receipt outcome `{outcome_kind}`"
        )));
    }
    let failure_id: Option<String> = row
        .try_get("failure_id")
        .map_err(|error| protocol_storage_error::<DB>("decode receipt failure ID", error))?;
    if (outcome_kind == "applied") != failure_id.is_none() {
        return Err(corrupt_storage(
            "projection receipt outcome/failure shape is inconsistent",
        ));
    }
    let change_epoch: String = row
        .try_get("change_epoch")
        .map_err(|error| protocol_storage_error::<DB>("decode receipt change epoch", error))?;
    let change_position = from_i64::<DB>(
        row.try_get("change_position").map_err(|error| {
            protocol_storage_error::<DB>("decode receipt change position", error)
        })?,
        "receipt change position",
    )?;
    let change = ProjectionChangeCursor::new(
        topology.clone(),
        partition.clone(),
        ProjectionEpoch::new(change_epoch)?,
        change_position,
    )?;
    Ok(StoredReceipt {
        source_bytes,
        source_hash,
        source_partition_bytes,
        source_partition_hash,
        source_epoch,
        source_position,
        input_fingerprint: ProjectionInputFingerprint::from_digest(input_hash),
        message_id,
        causation_id,
        gap_free,
        outcome_kind,
        change,
    })
}

fn receipt_matches_input(receipt: &StoredReceipt, input: &TrustedProjectionInput) -> bool {
    let source = input.cursor.source();
    receipt.source_bytes == source.canonical_name_bytes()
        && receipt.source_hash == digest_bytes(source.digest())
        && receipt.source_partition_bytes == source.canonical_partition_bytes()
        && receipt.source_partition_hash == digest_bytes(source.partition_digest())
        && receipt.source_epoch == *input.cursor.epoch()
        && receipt.source_position == input.cursor.position()
        && receipt.input_fingerprint == input.fingerprint
        && receipt.message_id == input.message_id
        && receipt.causation_id == input.causation_id
        && receipt.gap_free == input.gap_free
}

fn decode_stored_input_identity<DB>(
    row: &DB::Row,
) -> Result<StoredInputIdentity, ProjectionProtocolError>
where
    DB: SqlxRepoBackend,
    for<'q> i64: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> String: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> Vec<u8>: Type<DB> + sqlx::Decode<'q, DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    let partition_bytes: Vec<u8> = row.try_get("partition_bytes").map_err(|error| {
        protocol_storage_error::<DB>("decode input identity partition bytes", error)
    })?;
    let partition_hash: Vec<u8> = row.try_get("partition_hash").map_err(|error| {
        protocol_storage_error::<DB>("decode input identity partition hash", error)
    })?;
    let decoded_partition = ProjectionPartition::new(partition_bytes.clone())?;
    verify_digest(
        &partition_hash,
        decoded_partition.digest(),
        "projection input identity partition",
    )?;
    let source_bytes: Vec<u8> = row.try_get("source_bytes").map_err(|error| {
        protocol_storage_error::<DB>("decode input identity source bytes", error)
    })?;
    let source_hash: Vec<u8> = row.try_get("source_hash").map_err(|error| {
        protocol_storage_error::<DB>("decode input identity source hash", error)
    })?;
    let source_partition_bytes: Vec<u8> =
        row.try_get("source_partition_bytes").map_err(|error| {
            protocol_storage_error::<DB>("decode input identity source partition bytes", error)
        })?;
    let source_partition_hash: Vec<u8> = row.try_get("source_partition_hash").map_err(|error| {
        protocol_storage_error::<DB>("decode input identity source partition hash", error)
    })?;
    let source =
        ProjectionSource::from_canonical_name_bytes(&source_bytes, source_partition_bytes.clone())?;
    verify_digest(
        &source_hash,
        source.digest(),
        "projection input identity source",
    )?;
    verify_digest(
        &source_partition_hash,
        source.partition_digest(),
        "projection input identity source partition",
    )?;
    let source_epoch: String = row
        .try_get("source_epoch")
        .map_err(|error| protocol_storage_error::<DB>("decode input identity epoch", error))?;
    let source_position = from_i64::<DB>(
        row.try_get("source_position").map_err(|error| {
            protocol_storage_error::<DB>("decode input identity position", error)
        })?,
        "projection input identity position",
    )?;
    let input_hash = decode_digest(
        row.try_get("input_hash").map_err(|error| {
            protocol_storage_error::<DB>("decode input identity fingerprint", error)
        })?,
        "projection input identity",
    )?;
    let gap_free = match row
        .try_get::<i64, _>("gap_free")
        .map_err(|error| protocol_storage_error::<DB>("decode input identity gap flag", error))?
    {
        0 => false,
        1 => true,
        value => {
            return Err(corrupt_storage(format!(
                "input identity gap-free flag contains invalid value {value}"
            )))
        }
    };
    Ok(StoredInputIdentity {
        partition_bytes,
        partition_hash,
        source_bytes,
        source_hash,
        source_partition_bytes,
        source_partition_hash,
        source_epoch: ProjectionEpoch::new(source_epoch)?,
        source_position,
        input_fingerprint: ProjectionInputFingerprint::from_digest(input_hash),
        message_id: row.try_get("message_id").map_err(|error| {
            protocol_storage_error::<DB>("decode input identity message ID", error)
        })?,
        causation_id: row.try_get("causation_id").map_err(|error| {
            protocol_storage_error::<DB>("decode input identity causation ID", error)
        })?,
        gap_free,
    })
}

fn input_identity_cursor_matches(
    identity: &StoredInputIdentity,
    input: &TrustedProjectionInput,
) -> bool {
    let source = input.cursor.source();
    identity.partition_bytes == input.cursor.projection_partition().canonical_bytes()
        && identity.partition_hash == digest_bytes(input.cursor.projection_partition().digest())
        && identity.source_bytes == source.canonical_name_bytes()
        && identity.source_hash == digest_bytes(source.digest())
        && identity.source_partition_bytes == source.canonical_partition_bytes()
        && identity.source_partition_hash == digest_bytes(source.partition_digest())
        && identity.source_epoch == *input.cursor.epoch()
        && identity.source_position == input.cursor.position()
}

fn input_identity_matches(identity: &StoredInputIdentity, input: &TrustedProjectionInput) -> bool {
    input_identity_cursor_matches(identity, input)
        && identity.input_fingerprint == input.fingerprint
        && identity.message_id == input.message_id
        && identity.causation_id == input.causation_id
        && identity.gap_free == input.gap_free
}

async fn input_identity_by_cursor_in_tx<DB>(
    tx: &mut Transaction<'_, DB>,
    input: &TrustedProjectionInput,
) -> Result<Option<StoredInputIdentity>, ProjectionProtocolError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> String: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> Vec<u8>: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
    for<'q> &'q [u8]: Encode<'q, DB> + Type<DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    let topology_hash = input.cursor.topology().digest();
    let partition_hash = input.cursor.projection_partition().digest();
    let source_hash = input.cursor.source().digest();
    let source_partition_hash = input.cursor.source().partition_digest();
    let mut builder = QueryBuilder::<DB>::new(
        "SELECT partition.partition_bytes, identity.partition_hash, identity.source_bytes, \
         identity.source_hash, identity.source_partition_bytes, identity.source_partition_hash, \
         identity.source_epoch, identity.source_position, identity.input_hash, \
         identity.message_id, identity.causation_id, identity.gap_free \
         FROM projection_input_identities identity JOIN projection_partitions partition \
         ON partition.topology_hash = identity.topology_hash \
         AND partition.partition_hash = identity.partition_hash \
         WHERE identity.topology_hash = ",
    );
    builder.push_bind(topology_hash.as_slice());
    builder.push(" AND identity.partition_hash = ");
    builder.push_bind(partition_hash.as_slice());
    builder.push(" AND identity.source_hash = ");
    builder.push_bind(source_hash.as_slice());
    builder.push(" AND identity.source_partition_hash = ");
    builder.push_bind(source_partition_hash.as_slice());
    builder.push(" AND identity.source_epoch = ");
    builder.push_bind(input.cursor.epoch().as_str());
    builder.push(" AND identity.source_position = ");
    builder.push_bind(to_i64::<DB>(
        input.cursor.position(),
        "projection input identity position",
    )?);
    let row = builder
        .build()
        .fetch_optional(&mut **tx)
        .await
        .map_err(|error| {
            protocol_storage_error::<DB>("load projection input identity by cursor", error)
        })?;
    row.map(|row| decode_stored_input_identity::<DB>(&row))
        .transpose()
}

async fn input_identity_by_message_in_tx<DB>(
    tx: &mut Transaction<'_, DB>,
    input: &TrustedProjectionInput,
) -> Result<Option<StoredInputIdentity>, ProjectionProtocolError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> String: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> Vec<u8>: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
    for<'q> &'q [u8]: Encode<'q, DB> + Type<DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    let topology_hash = input.cursor.topology().digest();
    let mut builder = QueryBuilder::<DB>::new(
        "SELECT partition.partition_bytes, identity.partition_hash, identity.source_bytes, \
         identity.source_hash, identity.source_partition_bytes, identity.source_partition_hash, \
         identity.source_epoch, identity.source_position, identity.input_hash, \
         identity.message_id, identity.causation_id, identity.gap_free \
         FROM projection_input_identities identity JOIN projection_partitions partition \
         ON partition.topology_hash = identity.topology_hash \
         AND partition.partition_hash = identity.partition_hash \
         WHERE identity.topology_hash = ",
    );
    builder.push_bind(topology_hash.as_slice());
    builder.push(" AND identity.message_id = ");
    builder.push_bind(input.message_id.as_str());
    let row = builder
        .build()
        .fetch_optional(&mut **tx)
        .await
        .map_err(|error| {
            protocol_storage_error::<DB>("load projection input identity by message", error)
        })?;
    row.map(|row| decode_stored_input_identity::<DB>(&row))
        .transpose()
}

async fn receipt_by_message_in_tx<DB>(
    tx: &mut Transaction<'_, DB>,
    input: &TrustedProjectionInput,
) -> Result<Option<StoredReceipt>, ProjectionProtocolError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> String: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> Vec<u8>: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
    for<'q> &'q [u8]: Encode<'q, DB> + Type<DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    let generation = to_i64::<DB>(input.generation.get(), "projection generation")?;
    let topology_hash = input.cursor.topology().digest();
    let partition_hash = input.cursor.projection_partition().digest();
    let mut builder = QueryBuilder::<DB>::new(
        "SELECT source_bytes, source_hash, source_partition_bytes, source_partition_hash, \
         source_epoch, source_position, input_hash, message_id, causation_id, outcome_kind, failure_id, \
         gap_free, change_epoch, change_position FROM projection_input_receipts WHERE topology_hash = ",
    );
    builder.push_bind(topology_hash.as_slice());
    builder.push(" AND partition_hash = ");
    builder.push_bind(partition_hash.as_slice());
    builder.push(" AND generation = ");
    builder.push_bind(generation);
    builder.push(" AND message_id = ");
    builder.push_bind(input.message_id.as_str());
    let row = builder
        .build()
        .fetch_optional(&mut **tx)
        .await
        .map_err(|error| protocol_storage_error::<DB>("load projection message receipt", error))?;
    row.map(|row| {
        decode_stored_receipt::<DB>(
            &row,
            input.cursor.topology(),
            input.cursor.projection_partition(),
        )
    })
    .transpose()
}

async fn receipt_by_cursor_in_tx<DB>(
    tx: &mut Transaction<'_, DB>,
    input: &TrustedProjectionInput,
) -> Result<Option<StoredReceipt>, ProjectionProtocolError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> String: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> Vec<u8>: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
    for<'q> &'q [u8]: Encode<'q, DB> + Type<DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    let generation = to_i64::<DB>(input.generation.get(), "projection generation")?;
    let position = to_i64::<DB>(input.cursor.position(), "projection input position")?;
    let topology_hash = input.cursor.topology().digest();
    let partition_hash = input.cursor.projection_partition().digest();
    let source_hash = input.cursor.source().digest();
    let source_partition_hash = input.cursor.source().partition_digest();
    let mut builder = QueryBuilder::<DB>::new(
        "SELECT source_bytes, source_hash, source_partition_bytes, source_partition_hash, \
         source_epoch, source_position, input_hash, message_id, causation_id, outcome_kind, failure_id, \
         gap_free, change_epoch, change_position FROM projection_input_receipts WHERE topology_hash = ",
    );
    builder.push_bind(topology_hash.as_slice());
    builder.push(" AND partition_hash = ");
    builder.push_bind(partition_hash.as_slice());
    builder.push(" AND generation = ");
    builder.push_bind(generation);
    builder.push(" AND source_hash = ");
    builder.push_bind(source_hash.as_slice());
    builder.push(" AND source_partition_hash = ");
    builder.push_bind(source_partition_hash.as_slice());
    builder.push(" AND source_epoch = ");
    builder.push_bind(input.cursor.epoch().as_str());
    builder.push(" AND source_position = ");
    builder.push_bind(position);
    let row = builder
        .build()
        .fetch_optional(&mut **tx)
        .await
        .map_err(|error| protocol_storage_error::<DB>("load projection cursor receipt", error))?;
    row.map(|row| {
        decode_stored_receipt::<DB>(
            &row,
            input.cursor.topology(),
            input.cursor.projection_partition(),
        )
    })
    .transpose()
}

async fn current_input_cursor_in_tx<DB>(
    tx: &mut Transaction<'_, DB>,
    input: &TrustedProjectionInput,
) -> Result<Option<StoredCursor>, ProjectionProtocolError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> String: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> Vec<u8>: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> &'q [u8]: Encode<'q, DB> + Type<DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    let generation = to_i64::<DB>(input.generation.get(), "projection generation")?;
    let topology_hash = input.cursor.topology().digest();
    let partition_hash = input.cursor.projection_partition().digest();
    let source_hash = input.cursor.source().digest();
    let source_partition_hash = input.cursor.source().partition_digest();
    let mut builder = QueryBuilder::<DB>::new(
        "SELECT source_bytes, source_hash, source_partition_bytes, source_partition_hash, \
         source_epoch, source_position, input_hash, message_id, causation_id, \
         gap_free, change_epoch, change_position FROM projection_input_cursors WHERE topology_hash = ",
    );
    builder.push_bind(topology_hash.as_slice());
    builder.push(" AND partition_hash = ");
    builder.push_bind(partition_hash.as_slice());
    builder.push(" AND source_hash = ");
    builder.push_bind(source_hash.as_slice());
    builder.push(" AND source_partition_hash = ");
    builder.push_bind(source_partition_hash.as_slice());
    builder.push(" AND generation = ");
    builder.push_bind(generation);
    let Some(row) = builder
        .build()
        .fetch_optional(&mut **tx)
        .await
        .map_err(|error| protocol_storage_error::<DB>("load projection input cursor", error))?
    else {
        return Ok(None);
    };

    let source_bytes: Vec<u8> = row
        .try_get("source_bytes")
        .map_err(|error| protocol_storage_error::<DB>("decode cursor source bytes", error))?;
    let source_digest: Vec<u8> = row
        .try_get("source_hash")
        .map_err(|error| protocol_storage_error::<DB>("decode cursor source hash", error))?;
    let source_partition_bytes: Vec<u8> =
        row.try_get("source_partition_bytes").map_err(|error| {
            protocol_storage_error::<DB>("decode cursor source partition bytes", error)
        })?;
    let source_partition_digest: Vec<u8> =
        row.try_get("source_partition_hash").map_err(|error| {
            protocol_storage_error::<DB>("decode cursor source partition hash", error)
        })?;
    verify_bytes(
        &source_bytes,
        &input.cursor.source().canonical_name_bytes(),
        "projection cursor source",
    )?;
    verify_digest(
        &source_digest,
        input.cursor.source().digest(),
        "projection cursor source",
    )?;
    verify_bytes(
        &source_partition_bytes,
        input.cursor.source().canonical_partition_bytes(),
        "projection cursor source partition",
    )?;
    verify_digest(
        &source_partition_digest,
        input.cursor.source().partition_digest(),
        "projection cursor source partition",
    )?;
    let source_epoch: String = row
        .try_get("source_epoch")
        .map_err(|error| protocol_storage_error::<DB>("decode cursor source epoch", error))?;
    let source_position = from_i64::<DB>(
        row.try_get("source_position").map_err(|error| {
            protocol_storage_error::<DB>("decode cursor source position", error)
        })?,
        "projection input position",
    )?;
    let input_hash = decode_digest(
        row.try_get("input_hash")
            .map_err(|error| protocol_storage_error::<DB>("decode cursor input hash", error))?,
        "projection input",
    )?;
    let change_epoch: String = row
        .try_get("change_epoch")
        .map_err(|error| protocol_storage_error::<DB>("decode cursor change epoch", error))?;
    let change_position = from_i64::<DB>(
        row.try_get("change_position").map_err(|error| {
            protocol_storage_error::<DB>("decode cursor change position", error)
        })?,
        "projection change position",
    )?;
    Ok(Some(StoredCursor {
        source_epoch: ProjectionEpoch::new(source_epoch)?,
        source_position,
        input_fingerprint: ProjectionInputFingerprint::from_digest(input_hash),
        message_id: row
            .try_get("message_id")
            .map_err(|error| protocol_storage_error::<DB>("decode cursor message ID", error))?,
        causation_id: row
            .try_get("causation_id")
            .map_err(|error| protocol_storage_error::<DB>("decode cursor causation ID", error))?,
        gap_free: match row
            .try_get::<i64, _>("gap_free")
            .map_err(|error| protocol_storage_error::<DB>("decode cursor gap-free flag", error))?
        {
            0 => false,
            1 => true,
            value => {
                return Err(corrupt_storage(format!(
                    "cursor gap-free flag contains invalid value {value}"
                )))
            }
        },
        change: ProjectionChangeCursor::new(
            input.cursor.topology().clone(),
            input.cursor.projection_partition().clone(),
            ProjectionEpoch::new(change_epoch)?,
            change_position,
        )?,
    }))
}

fn stored_cursors_match(left: &StoredCursor, right: &StoredCursor) -> bool {
    left.source_epoch == right.source_epoch
        && left.source_position == right.source_position
        && left.input_fingerprint == right.input_fingerprint
        && left.message_id == right.message_id
        && left.causation_id == right.causation_id
        && left.gap_free == right.gap_free
        && left.change == right.change
}

async fn verify_inherited_cursor_in_tx<DB>(
    tx: &mut Transaction<'_, DB>,
    input: &TrustedProjectionInput,
    current: &StoredCursor,
) -> Result<bool, ProjectionProtocolError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> String: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> Vec<u8>: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> &'q [u8]: Encode<'q, DB> + Type<DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    let topology_hash = input.cursor.topology().digest();
    let partition_hash = input.cursor.projection_partition().digest();
    let mut builder = QueryBuilder::<DB>::new(
        "SELECT retry_of_generation FROM projection_generations WHERE topology_hash = ",
    );
    builder.push_bind(topology_hash.as_slice());
    builder.push(" AND partition_hash = ");
    builder.push_bind(partition_hash.as_slice());
    builder.push(" AND generation = ");
    builder.push_bind(to_i64::<DB>(
        input.generation.get(),
        "projection generation",
    )?);
    let row = builder
        .build()
        .fetch_optional(&mut **tx)
        .await
        .map_err(|error| {
            protocol_storage_error::<DB>("load projection repair generation lineage", error)
        })?
        .ok_or_else(|| {
            corrupt_storage(format!(
                "active projection generation {} is missing",
                input.generation.get()
            ))
        })?;
    let parent: Option<i64> = row.try_get("retry_of_generation").map_err(|error| {
        protocol_storage_error::<DB>("decode projection repair generation lineage", error)
    })?;
    let Some(parent) = parent else {
        return Ok(false);
    };
    let parent = ProjectionGeneration::new(from_i64::<DB>(
        parent,
        "projection repair parent generation",
    )?)?;
    let mut parent_input = input.clone();
    parent_input.generation = parent;
    let parent_cursor = current_input_cursor_in_tx(tx, &parent_input)
        .await?
        .ok_or_else(|| {
            corrupt_storage("projection repair generation contains a cursor absent from its parent")
        })?;
    if !stored_cursors_match(current, &parent_cursor) {
        return Err(corrupt_storage(
            "projection repair generation contains a cursor changed from its parent",
        ));
    }
    Ok(true)
}

async fn validate_source_capability_in_tx_mode<DB>(
    tx: &mut Transaction<'_, DB>,
    input: &TrustedProjectionInput,
    register_missing: bool,
) -> Result<(), ProjectionProtocolError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> Vec<u8>: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> &'q [u8]: Encode<'q, DB> + Type<DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    let topology_hash = input.cursor.topology().digest();
    let partition_hash = input.cursor.projection_partition().digest();
    let source_hash = input.cursor.source().digest();
    let source_partition_hash = input.cursor.source().partition_digest();
    let mut builder = QueryBuilder::<DB>::new(
        "SELECT source_bytes, source_hash, source_partition_bytes, source_partition_hash, gap_free \
         FROM projection_source_capabilities WHERE topology_hash = ",
    );
    builder.push_bind(topology_hash.as_slice());
    builder.push(" AND partition_hash = ");
    builder.push_bind(partition_hash.as_slice());
    builder.push(" AND source_hash = ");
    builder.push_bind(source_hash.as_slice());
    builder.push(" AND source_partition_hash = ");
    builder.push_bind(source_partition_hash.as_slice());
    builder.push(" LIMIT 1");
    let row = builder
        .build()
        .fetch_optional(&mut **tx)
        .await
        .map_err(|error| {
            protocol_storage_error::<DB>("load projection source capability", error)
        })?;
    let Some(row) = row else {
        if !register_missing {
            return Ok(());
        }
        let source = input.cursor.source();
        let source_bytes = source.canonical_name_bytes();
        let source_hash = source.digest();
        let source_partition_hash = source.partition_digest();
        let mut insert = QueryBuilder::<DB>::new(
            "INSERT INTO projection_source_capabilities \
             (topology_hash, partition_hash, source_bytes, source_hash, source_partition_bytes, \
             source_partition_hash, gap_free) VALUES (",
        );
        insert.push_bind(topology_hash.as_slice());
        insert.push(", ");
        insert.push_bind(partition_hash.as_slice());
        insert.push(", ");
        insert.push_bind(source_bytes.as_slice());
        insert.push(", ");
        insert.push_bind(source_hash.as_slice());
        insert.push(", ");
        insert.push_bind(source.canonical_partition_bytes());
        insert.push(", ");
        insert.push_bind(source_partition_hash.as_slice());
        insert.push(", ");
        insert.push_bind(i64::from(input.gap_free));
        insert.push(
            ") ON CONFLICT \
             (topology_hash, partition_hash, source_hash, source_partition_hash) DO NOTHING",
        );
        let result = insert.build().execute(&mut **tx).await.map_err(|error| {
            protocol_storage_error::<DB>("register projection source capability", error)
        })?;
        if DB::rows_affected(&result) != 1 {
            return Err(corrupt_storage(
                "projection source capability changed while its partition lock was held",
            ));
        }
        return Ok(());
    };
    let source_bytes: Vec<u8> = row
        .try_get("source_bytes")
        .map_err(|error| protocol_storage_error::<DB>("decode capability source bytes", error))?;
    let source_digest: Vec<u8> = row
        .try_get("source_hash")
        .map_err(|error| protocol_storage_error::<DB>("decode capability source hash", error))?;
    let source_partition_bytes: Vec<u8> =
        row.try_get("source_partition_bytes").map_err(|error| {
            protocol_storage_error::<DB>("decode capability source partition bytes", error)
        })?;
    let source_partition_digest: Vec<u8> =
        row.try_get("source_partition_hash").map_err(|error| {
            protocol_storage_error::<DB>("decode capability source partition hash", error)
        })?;
    verify_bytes(
        &source_bytes,
        &input.cursor.source().canonical_name_bytes(),
        "projection capability source",
    )?;
    verify_digest(
        &source_digest,
        input.cursor.source().digest(),
        "projection capability source",
    )?;
    verify_bytes(
        &source_partition_bytes,
        input.cursor.source().canonical_partition_bytes(),
        "projection capability source partition",
    )?;
    verify_digest(
        &source_partition_digest,
        input.cursor.source().partition_digest(),
        "projection capability source partition",
    )?;
    let gap_free = match row.try_get::<i64, _>("gap_free").map_err(|error| {
        protocol_storage_error::<DB>("decode projection source capability", error)
    })? {
        0 => false,
        1 => true,
        value => {
            return Err(corrupt_storage(format!(
                "source capability gap-free flag contains invalid value {value}"
            )))
        }
    };
    if gap_free != input.gap_free {
        return Err(ProjectionProtocolError::InputCorruption);
    }
    Ok(())
}

async fn validate_input_identity_in_tx_mode<DB>(
    tx: &mut Transaction<'_, DB>,
    input: &TrustedProjectionInput,
    register_missing_source_capability: bool,
) -> Result<(), ProjectionProtocolError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> String: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> Vec<u8>: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
    for<'q> &'q [u8]: Encode<'q, DB> + Type<DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    validate_source_capability_in_tx_mode(tx, input, register_missing_source_capability).await?;
    let exact_identity = input_identity_by_cursor_in_tx(tx, input).await?;
    if let Some(identity) = &exact_identity {
        if !input_identity_cursor_matches(identity, input) {
            return Err(corrupt_storage(
                "projection input identity hash lookup resolved different canonical source bytes",
            ));
        }
        if !input_identity_matches(identity, input) {
            return Err(ProjectionProtocolError::InputCorruption);
        }
    }
    if let Some(identity) = input_identity_by_message_in_tx(tx, input).await? {
        if !input_identity_cursor_matches(&identity, input) {
            return Err(ProjectionProtocolError::MessageIdReuse {
                message_id: input.message_id.clone(),
            });
        }
        if !input_identity_matches(&identity, input) {
            return Err(ProjectionProtocolError::InputCorruption);
        }
        if exact_identity.is_none() {
            return Err(corrupt_storage(
                "projection message identity exists without its exact cursor identity",
            ));
        }
    }
    Ok(())
}

async fn validate_input_identity_in_tx<DB>(
    tx: &mut Transaction<'_, DB>,
    input: &TrustedProjectionInput,
) -> Result<(), ProjectionProtocolError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> String: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> Vec<u8>: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
    for<'q> &'q [u8]: Encode<'q, DB> + Type<DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    validate_input_identity_in_tx_mode(tx, input, true).await
}

async fn validate_input_identity_read_only_in_tx<DB>(
    tx: &mut Transaction<'_, DB>,
    input: &TrustedProjectionInput,
) -> Result<(), ProjectionProtocolError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> String: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> Vec<u8>: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
    for<'q> &'q [u8]: Encode<'q, DB> + Type<DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    validate_input_identity_in_tx_mode(tx, input, false).await
}

async fn classify_validated_input_in_tx<DB>(
    tx: &mut Transaction<'_, DB>,
    input: &TrustedProjectionInput,
    state: &PartitionState,
) -> Result<InputDisposition, ProjectionProtocolError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> String: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> Vec<u8>: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
    for<'q> &'q [u8]: Encode<'q, DB> + Type<DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    if let Some(receipt) = receipt_by_cursor_in_tx(tx, input).await? {
        verify_stored_change(state, &receipt.change)?;
        if receipt.source_bytes != input.cursor.source().canonical_name_bytes()
            || receipt.source_hash != digest_bytes(input.cursor.source().digest())
            || receipt.source_partition_bytes != input.cursor.source().canonical_partition_bytes()
            || receipt.source_partition_hash
                != digest_bytes(input.cursor.source().partition_digest())
        {
            return Err(corrupt_storage(
                "projection cursor receipt hash lookup resolved different canonical bytes",
            ));
        }
        if receipt.input_fingerprint != input.fingerprint
            || receipt.message_id != input.message_id
            || receipt.causation_id != input.causation_id
            || receipt.gap_free != input.gap_free
        {
            return Err(ProjectionProtocolError::InputCorruption);
        }
        if receipt.outcome_kind != "applied" {
            return Err(corrupt_storage(
                "failed cursor receipt exists without a stopped partition",
            ));
        }
        return Ok(InputDisposition::Duplicate(checkpoint_from_stored(
            &input.cursor,
            receipt.source_epoch,
            receipt.source_position,
            receipt.change,
            receipt.gap_free,
        )?));
    }

    if let Some(receipt) = receipt_by_message_in_tx(tx, input).await? {
        verify_stored_change(state, &receipt.change)?;
        if !receipt_matches_input(&receipt, input) {
            return Err(ProjectionProtocolError::MessageIdReuse {
                message_id: input.message_id.clone(),
            });
        }
        if receipt.outcome_kind != "applied" {
            return Err(corrupt_storage(
                "failed input receipt exists without a stopped partition",
            ));
        }
        return Ok(InputDisposition::Duplicate(checkpoint_from_stored(
            &input.cursor,
            receipt.source_epoch,
            receipt.source_position,
            receipt.change,
            receipt.gap_free,
        )?));
    }

    let Some(previous) = current_input_cursor_in_tx(tx, input).await? else {
        return Ok(InputDisposition::New);
    };
    verify_stored_change(state, &previous.change)?;
    if previous.gap_free != input.gap_free {
        return Err(ProjectionProtocolError::InputCorruption);
    }
    if previous.source_epoch != *input.cursor.epoch() {
        return Err(ProjectionProtocolError::IncomparableInput);
    }
    if input.cursor.position() < previous.source_position {
        return Ok(InputDisposition::Stale(checkpoint_from_stored(
            &input.cursor,
            previous.source_epoch,
            previous.source_position,
            previous.change,
            previous.gap_free,
        )?));
    }
    if input.cursor.position() == previous.source_position {
        if previous.input_fingerprint != input.fingerprint
            || previous.message_id != input.message_id
            || previous.causation_id != input.causation_id
            || previous.gap_free != input.gap_free
        {
            return Err(ProjectionProtocolError::InputCorruption);
        }
        if !verify_inherited_cursor_in_tx(tx, input, &previous).await? {
            return Err(corrupt_storage(
                "projection input cursor has no receipt and was not inherited by repair",
            ));
        }
        return Ok(InputDisposition::Duplicate(checkpoint_from_stored(
            &input.cursor,
            previous.source_epoch,
            previous.source_position,
            previous.change,
            previous.gap_free,
        )?));
    }
    if input.gap_free
        && input.cursor.position()
            != checked_next(previous.source_position, "gap-free projection input")?
    {
        return Err(ProjectionProtocolError::IncomparableInput);
    }
    Ok(InputDisposition::New)
}

async fn ensure_partition_ownership_in_tx<DB>(
    tx: &mut Transaction<'_, DB>,
    topology: &ProjectorTopologyId,
    partition: &ProjectionPartition,
    ownership: &[ProjectionModelOwnership],
) -> Result<(), ProjectionProtocolError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> String: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> Vec<u8>: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
    for<'q> &'q [u8]: Encode<'q, DB> + Type<DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    let topology_hash = topology.digest();
    let partition_hash = partition.digest();
    for declaration in ownership {
        let mut registered = QueryBuilder::<DB>::new(
            "SELECT topology_bytes, table_name FROM projection_registered_models \
             WHERE topology_hash = ",
        );
        registered.push_bind(topology_hash.as_slice());
        registered.push(" AND model_name = ");
        registered.push_bind(declaration.model.as_str());
        let Some(row) = registered
            .build()
            .fetch_optional(&mut **tx)
            .await
            .map_err(|error| {
                protocol_storage_error::<DB>("verify causal projection registration", error)
            })?
        else {
            return Err(ProjectionProtocolError::InvalidBatch(format!(
                "projection model `{}` was not registered before projector traffic",
                declaration.model
            )));
        };
        let registered_topology: Vec<u8> = row.try_get("topology_bytes").map_err(|error| {
            protocol_storage_error::<DB>("decode causal projection registration", error)
        })?;
        verify_bytes(
            &registered_topology,
            &topology.canonical_bytes(),
            "registered projector topology",
        )?;
        let registered_table: String = row.try_get("table_name").map_err(|error| {
            protocol_storage_error::<DB>("decode causal projection registration", error)
        })?;
        if registered_table != declaration.table {
            return Err(ProjectionProtocolError::InvalidBatch(format!(
                "projection model `{}` was registered for table `{registered_table}`, not `{}`",
                declaration.model, declaration.table
            )));
        }

        let mut by_model = QueryBuilder::<DB>::new(
            "SELECT table_name FROM projection_model_ownership WHERE topology_hash = ",
        );
        by_model.push_bind(topology_hash.as_slice());
        by_model.push(" AND partition_hash = ");
        by_model.push_bind(partition_hash.as_slice());
        by_model.push(" AND model_name = ");
        by_model.push_bind(declaration.model.as_str());
        if let Some(row) = by_model
            .build()
            .fetch_optional(&mut **tx)
            .await
            .map_err(|error| {
                protocol_storage_error::<DB>("load projection model ownership", error)
            })?
        {
            let table: String = row.try_get("table_name").map_err(|error| {
                protocol_storage_error::<DB>("decode projection model ownership", error)
            })?;
            if table != declaration.table {
                return Err(ProjectionProtocolError::InvalidBatch(format!(
                    "projection model `{}` is already bound to table `{table}`",
                    declaration.model
                )));
            }
            continue;
        }

        let mut by_table = QueryBuilder::<DB>::new(
            "SELECT model_name FROM projection_model_ownership WHERE topology_hash = ",
        );
        by_table.push_bind(topology_hash.as_slice());
        by_table.push(" AND partition_hash = ");
        by_table.push_bind(partition_hash.as_slice());
        by_table.push(" AND table_name = ");
        by_table.push_bind(declaration.table.as_str());
        if let Some(row) = by_table
            .build()
            .fetch_optional(&mut **tx)
            .await
            .map_err(|error| {
                protocol_storage_error::<DB>("load projection table ownership", error)
            })?
        {
            let model: String = row.try_get("model_name").map_err(|error| {
                protocol_storage_error::<DB>("decode projection table ownership", error)
            })?;
            return Err(ProjectionProtocolError::InvalidBatch(format!(
                "projection table `{}` is already bound to model `{model}`",
                declaration.table
            )));
        }

        let mut insert = QueryBuilder::<DB>::new(
            "INSERT INTO projection_model_ownership \
             (topology_hash, partition_hash, model_name, table_name) VALUES (",
        );
        insert.push_bind(topology_hash.as_slice());
        insert.push(", ");
        insert.push_bind(partition_hash.as_slice());
        insert.push(", ");
        insert.push_bind(declaration.model.as_str());
        insert.push(", ");
        insert.push_bind(declaration.table.as_str());
        insert.push(") ON CONFLICT (topology_hash, partition_hash, model_name) DO NOTHING");
        let result = insert.build().execute(&mut **tx).await.map_err(|error| {
            protocol_storage_error::<DB>("insert projection model ownership", error)
        })?;
        if DB::rows_affected(&result) != 1 {
            return Err(corrupt_storage(
                "projection ownership changed while its partition lock was held",
            ));
        }
    }
    Ok(())
}

async fn verify_registered_topology_in_tx<DB>(
    tx: &mut Transaction<'_, DB>,
    topology: &ProjectorTopologyId,
) -> Result<(), ProjectionProtocolError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> Vec<u8>: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> &'q [u8]: Encode<'q, DB> + Type<DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    let topology_hash = topology.digest();
    let mut builder = QueryBuilder::<DB>::new(
        "SELECT topology_bytes FROM projection_registered_models WHERE topology_hash = ",
    );
    builder.push_bind(topology_hash.as_slice());
    builder.push(" LIMIT 1");
    let Some(row) = builder
        .build()
        .fetch_optional(&mut **tx)
        .await
        .map_err(|error| {
            protocol_storage_error::<DB>("verify registered projector topology", error)
        })?
    else {
        return Err(ProjectionProtocolError::InvalidBatch(format!(
            "projector topology `{}` has no registered model set",
            topology.name()
        )));
    };
    let topology_bytes: Vec<u8> = row.try_get("topology_bytes").map_err(|error| {
        protocol_storage_error::<DB>("decode registered projector topology", error)
    })?;
    verify_bytes(
        &topology_bytes,
        &topology.canonical_bytes(),
        "registered projector topology",
    )
}

async fn record_in_tx<DB>(
    tx: &mut Transaction<'_, DB>,
    scope: &ProjectionRecordScope,
    expected_change_epoch: &ProjectionEpoch,
) -> Result<Option<StoredRecord>, ProjectionProtocolError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> String: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> Vec<u8>: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
    for<'q> &'q [u8]: Encode<'q, DB> + Type<DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    let topology_hash = scope.topology().digest();
    let partition_hash = scope.projection_partition().digest();
    let key_hash = scope.key_digest();
    let mut builder = QueryBuilder::<DB>::new(
        "SELECT canonical_key_bytes, canonical_key_hash, incarnation, revision, tombstone, \
         change_epoch, change_position FROM projection_records WHERE topology_hash = ",
    );
    builder.push_bind(topology_hash.as_slice());
    builder.push(" AND partition_hash = ");
    builder.push_bind(partition_hash.as_slice());
    builder.push(" AND model_name = ");
    builder.push_bind(scope.model());
    builder.push(" AND canonical_key_hash = ");
    builder.push_bind(key_hash.as_slice());
    let Some(row) = builder
        .build()
        .fetch_optional(&mut **tx)
        .await
        .map_err(|error| protocol_storage_error::<DB>("load projection record", error))?
    else {
        return Ok(None);
    };
    let key_bytes: Vec<u8> = row.try_get("canonical_key_bytes").map_err(|error| {
        protocol_storage_error::<DB>("decode projection record key bytes", error)
    })?;
    let stored_key_hash: Vec<u8> = row.try_get("canonical_key_hash").map_err(|error| {
        protocol_storage_error::<DB>("decode projection record key hash", error)
    })?;
    verify_bytes(
        &key_bytes,
        scope.canonical_key_bytes(),
        "projection record key",
    )?;
    verify_digest(
        &stored_key_hash,
        scope.key_digest(),
        "projection record key",
    )?;
    let incarnation = from_i64::<DB>(
        row.try_get("incarnation")
            .map_err(|error| protocol_storage_error::<DB>("decode record incarnation", error))?,
        "record incarnation",
    )?;
    let revision = from_i64::<DB>(
        row.try_get("revision")
            .map_err(|error| protocol_storage_error::<DB>("decode record revision", error))?,
        "record revision",
    )?;
    let tombstone_value: i64 = row
        .try_get("tombstone")
        .map_err(|error| protocol_storage_error::<DB>("decode record tombstone", error))?;
    let tombstone = match tombstone_value {
        0 => false,
        1 => true,
        value => {
            return Err(corrupt_storage(format!(
                "record tombstone contains invalid value {value}"
            )))
        }
    };
    let change_epoch: String = row
        .try_get("change_epoch")
        .map_err(|error| protocol_storage_error::<DB>("decode record change epoch", error))?;
    let change_epoch = ProjectionEpoch::new(change_epoch)?;
    if &change_epoch != expected_change_epoch {
        return Err(corrupt_storage(
            "projection record change epoch differs from its partition",
        ));
    }
    let change_position = from_i64::<DB>(
        row.try_get("change_position").map_err(|error| {
            protocol_storage_error::<DB>("decode record change position", error)
        })?,
        "record change position",
    )?;
    Ok(Some(StoredRecord {
        metadata: ProjectionRecordMetadata {
            revision: RecordRevision::new(scope.clone(), incarnation, revision)?,
            tombstone,
            change: ProjectionChangeCursor::new(
                scope.topology().clone(),
                scope.projection_partition().clone(),
                change_epoch,
                change_position,
            )?,
        },
    }))
}

fn next_record(
    scope: &ProjectionRecordScope,
    expectation: &ProjectionRecordExpectation,
    kind: ProjectionMutationKind,
    current: Option<&StoredRecord>,
) -> Result<(RecordRevision, bool), ProjectionProtocolError> {
    let current = current.map(|record| &record.metadata);
    match (expectation, current, kind) {
        (ProjectionRecordExpectation::Missing, None, ProjectionMutationKind::Upsert) => {
            Ok((RecordRevision::new(scope.clone(), 1, 1)?, false))
        }
        (ProjectionRecordExpectation::Missing, Some(metadata), _) if metadata.tombstone => {
            Err(ProjectionProtocolError::RecordTombstoned {
                model: scope.model().to_string(),
            })
        }
        (ProjectionRecordExpectation::Missing, Some(_), _) => {
            Err(ProjectionProtocolError::RecordAlreadyExists {
                model: scope.model().to_string(),
            })
        }
        (ProjectionRecordExpectation::Exact(_), None, _) => {
            Err(ProjectionProtocolError::RecordMissing {
                model: scope.model().to_string(),
            })
        }
        (ProjectionRecordExpectation::Exact(expected), Some(metadata), _) => {
            if expected != &metadata.revision {
                return Err(ProjectionProtocolError::RecordRevisionConflict {
                    model: scope.model().to_string(),
                    expected_incarnation: expected.incarnation(),
                    expected_revision: expected.revision(),
                    actual_incarnation: metadata.revision.incarnation(),
                    actual_revision: metadata.revision.revision(),
                });
            }
            match kind {
                ProjectionMutationKind::Upsert if metadata.tombstone => {
                    Err(ProjectionProtocolError::RecordTombstoned {
                        model: scope.model().to_string(),
                    })
                }
                ProjectionMutationKind::Upsert => Ok((
                    RecordRevision::new(
                        scope.clone(),
                        metadata.revision.incarnation(),
                        checked_next(metadata.revision.revision(), "record revision")?,
                    )?,
                    false,
                )),
                ProjectionMutationKind::Delete if metadata.tombstone => {
                    Err(ProjectionProtocolError::RecordTombstoned {
                        model: scope.model().to_string(),
                    })
                }
                ProjectionMutationKind::Delete => Ok((
                    RecordRevision::new(
                        scope.clone(),
                        metadata.revision.incarnation(),
                        checked_next(metadata.revision.revision(), "record revision")?,
                    )?,
                    true,
                )),
                ProjectionMutationKind::Recreate if !metadata.tombstone => {
                    Err(ProjectionProtocolError::RecreateRequiresTombstone {
                        model: scope.model().to_string(),
                    })
                }
                ProjectionMutationKind::Recreate => Ok((
                    RecordRevision::new(
                        scope.clone(),
                        checked_next(metadata.revision.incarnation(), "record incarnation")?,
                        1,
                    )?,
                    false,
                )),
            }
        }
        (_, _, ProjectionMutationKind::Delete | ProjectionMutationKind::Recreate) => {
            Err(ProjectionProtocolError::InvalidBatch(
                "delete/recreate requires an exact record expectation".into(),
            ))
        }
    }
}

fn allocate_change(
    state: &mut PartitionState,
    topology: &ProjectorTopologyId,
    partition: &ProjectionPartition,
    kind: ProjectionChangeKind,
    causation_id: String,
    observation_kind: Option<ProjectionObservationKind>,
    scope: Option<ProjectionRecordScope>,
    revision: Option<RecordRevision>,
    failure_id: Option<String>,
) -> Result<ProjectionChange, ProjectionProtocolError> {
    state.change_head = checked_next(state.change_head, "projection change")?;
    Ok(ProjectionChange {
        cursor: ProjectionChangeCursor::new(
            topology.clone(),
            partition.clone(),
            state.change_epoch.clone(),
            state.change_head,
        )?,
        kind,
        causation_id,
        observation_kind,
        scope,
        revision,
        failure_id,
    })
}

async fn insert_change_in_tx<DB>(
    tx: &mut Transaction<'_, DB>,
    change: &ProjectionChange,
) -> Result<(), ProjectionProtocolError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
    for<'q> &'q [u8]: Encode<'q, DB> + Type<DB>,
{
    let cursor = &change.cursor;
    let position = to_i64::<DB>(cursor.position(), "projection change position")?;
    let topology_hash = cursor.topology().digest();
    let partition_hash = cursor.projection_partition().digest();
    let mut builder = QueryBuilder::<DB>::new(
        "INSERT INTO projection_changes \
         (topology_hash, partition_hash, change_epoch, change_position, change_kind, \
         causation_id, model_name, scope_kind, canonical_key_bytes, canonical_key_hash, \
         incarnation, revision, failure_id) VALUES (",
    );
    builder.push_bind(topology_hash.as_slice());
    builder.push(", ");
    builder.push_bind(partition_hash.as_slice());
    builder.push(", ");
    builder.push_bind(cursor.epoch().as_str());
    builder.push(", ");
    builder.push_bind(position);
    builder.push(", ");
    builder.push_bind(change.kind.as_storage_str());
    builder.push(", ");
    builder.push_bind(change.causation_id.as_str());
    builder.push(", ");
    if let Some(scope) = &change.scope {
        builder.push_bind(scope.model());
    } else {
        builder.push("NULL");
    }
    builder.push(", ");
    if let Some(kind) = change.observation_kind {
        builder.push_bind(kind.as_storage_str());
    } else {
        builder.push("NULL");
    }
    builder.push(", ");
    if let Some(scope) = &change.scope {
        builder.push_bind(scope.canonical_key_bytes());
    } else {
        builder.push("NULL");
    }
    builder.push(", ");
    if let Some(scope) = &change.scope {
        let key_hash = scope.key_digest();
        builder.push_bind(key_hash.as_slice());
    } else {
        builder.push("NULL");
    }
    builder.push(", ");
    if let Some(revision) = &change.revision {
        builder.push_bind(to_i64::<DB>(
            revision.incarnation(),
            "projection record incarnation",
        )?);
    } else {
        builder.push("NULL");
    }
    builder.push(", ");
    if let Some(revision) = &change.revision {
        builder.push_bind(to_i64::<DB>(
            revision.revision(),
            "projection record revision",
        )?);
    } else {
        builder.push("NULL");
    }
    builder.push(", ");
    if let Some(failure_id) = &change.failure_id {
        builder.push_bind(failure_id.as_str());
    } else {
        builder.push("NULL");
    }
    builder.push(")");
    builder
        .build()
        .execute(&mut **tx)
        .await
        .map_err(|error| protocol_storage_error::<DB>("append projection change", error))?;
    Ok(())
}

async fn upsert_record_in_tx<DB>(
    tx: &mut Transaction<'_, DB>,
    metadata: &ProjectionRecordMetadata,
) -> Result<(), ProjectionProtocolError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
    for<'q> &'q [u8]: Encode<'q, DB> + Type<DB>,
{
    let scope = metadata.revision.scope();
    let topology_hash = scope.topology().digest();
    let partition_hash = scope.projection_partition().digest();
    let key_hash = scope.key_digest();
    let mut builder = QueryBuilder::<DB>::new(
        "INSERT INTO projection_records \
         (topology_hash, partition_hash, model_name, canonical_key_bytes, canonical_key_hash, \
         incarnation, revision, tombstone, change_epoch, change_position) VALUES (",
    );
    builder.push_bind(topology_hash.as_slice());
    builder.push(", ");
    builder.push_bind(partition_hash.as_slice());
    builder.push(", ");
    builder.push_bind(scope.model());
    builder.push(", ");
    builder.push_bind(scope.canonical_key_bytes());
    builder.push(", ");
    builder.push_bind(key_hash.as_slice());
    builder.push(", ");
    builder.push_bind(to_i64::<DB>(
        metadata.revision.incarnation(),
        "projection record incarnation",
    )?);
    builder.push(", ");
    builder.push_bind(to_i64::<DB>(
        metadata.revision.revision(),
        "projection record revision",
    )?);
    builder.push(", ");
    builder.push_bind(i64::from(metadata.tombstone));
    builder.push(", ");
    builder.push_bind(metadata.change.epoch().as_str());
    builder.push(", ");
    builder.push_bind(to_i64::<DB>(
        metadata.change.position(),
        "projection record change position",
    )?);
    builder.push(
        ") ON CONFLICT (topology_hash, partition_hash, model_name, canonical_key_hash) \
         DO UPDATE SET canonical_key_bytes = excluded.canonical_key_bytes, \
         incarnation = excluded.incarnation, revision = excluded.revision, \
         tombstone = excluded.tombstone, change_epoch = excluded.change_epoch, \
         change_position = excluded.change_position",
    );
    builder
        .build()
        .execute(&mut **tx)
        .await
        .map_err(|error| protocol_storage_error::<DB>("store projection record", error))?;
    Ok(())
}

fn decode_observation_row<DB>(
    row: &DB::Row,
    causation_id: &str,
    scope: &ProjectionRecordScope,
    kind: ProjectionObservationKind,
    expected_change_epoch: &ProjectionEpoch,
) -> Result<ProjectionObservation, ProjectionProtocolError>
where
    DB: SqlxRepoBackend,
    for<'q> i64: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> String: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> Vec<u8>: Type<DB> + sqlx::Decode<'q, DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    let key_bytes: Vec<u8> = row
        .try_get("canonical_key_bytes")
        .map_err(|error| protocol_storage_error::<DB>("decode observation key bytes", error))?;
    let stored_key_hash: Vec<u8> = row
        .try_get("canonical_key_hash")
        .map_err(|error| protocol_storage_error::<DB>("decode observation key hash", error))?;
    verify_bytes(
        &key_bytes,
        scope.canonical_key_bytes(),
        "projection observation key",
    )?;
    verify_digest(
        &stored_key_hash,
        scope.key_digest(),
        "projection observation key",
    )?;
    let incarnation: Option<i64> = row
        .try_get("incarnation")
        .map_err(|error| protocol_storage_error::<DB>("decode observation incarnation", error))?;
    let revision: Option<i64> = row
        .try_get("revision")
        .map_err(|error| protocol_storage_error::<DB>("decode observation revision", error))?;
    let revision = match (kind, incarnation, revision) {
        (ProjectionObservationKind::Record, Some(incarnation), Some(revision)) => {
            Some(RecordRevision::new(
                scope.clone(),
                from_i64::<DB>(incarnation, "observation record incarnation")?,
                from_i64::<DB>(revision, "observation record revision")?,
            )?)
        }
        (ProjectionObservationKind::Dependency, None, None) => None,
        _ => {
            return Err(corrupt_storage(
                "projection observation kind/revision shape is inconsistent",
            ))
        }
    };
    let change_epoch: String = row
        .try_get("change_epoch")
        .map_err(|error| protocol_storage_error::<DB>("decode observation change epoch", error))?;
    let change_epoch = ProjectionEpoch::new(change_epoch)?;
    if &change_epoch != expected_change_epoch {
        return Err(corrupt_storage(
            "projection observation change epoch differs from its partition",
        ));
    }
    let change_position = from_i64::<DB>(
        row.try_get("change_position").map_err(|error| {
            protocol_storage_error::<DB>("decode observation change position", error)
        })?,
        "observation change position",
    )?;
    Ok(ProjectionObservation {
        causation_id: causation_id.to_string(),
        kind,
        revision,
        scope: scope.clone(),
        change: ProjectionChangeCursor::new(
            scope.topology().clone(),
            scope.projection_partition().clone(),
            change_epoch,
            change_position,
        )?,
    })
}

async fn observation_in_tx<DB>(
    tx: &mut Transaction<'_, DB>,
    causation_id: &str,
    scope: &ProjectionRecordScope,
    kind: ProjectionObservationKind,
    expected_change_epoch: &ProjectionEpoch,
) -> Result<Option<ProjectionObservation>, ProjectionProtocolError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> String: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> Vec<u8>: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
    for<'q> &'q [u8]: Encode<'q, DB> + Type<DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    let topology_hash = scope.topology().digest();
    let partition_hash = scope.projection_partition().digest();
    let key_hash = scope.key_digest();
    let mut builder = QueryBuilder::<DB>::new(
        "SELECT canonical_key_bytes, canonical_key_hash, incarnation, revision, \
         change_epoch, change_position FROM projection_observations WHERE topology_hash = ",
    );
    builder.push_bind(topology_hash.as_slice());
    builder.push(" AND partition_hash = ");
    builder.push_bind(partition_hash.as_slice());
    builder.push(" AND causation_id = ");
    builder.push_bind(causation_id);
    builder.push(" AND model_name = ");
    builder.push_bind(scope.model());
    builder.push(" AND scope_kind = ");
    builder.push_bind(kind.as_storage_str());
    builder.push(" AND canonical_key_hash = ");
    builder.push_bind(key_hash.as_slice());
    let Some(row) = builder
        .build()
        .fetch_optional(&mut **tx)
        .await
        .map_err(|error| protocol_storage_error::<DB>("load projection observation", error))?
    else {
        return Ok(None);
    };
    decode_observation_row::<DB>(&row, causation_id, scope, kind, expected_change_epoch).map(Some)
}

async fn insert_observation_in_tx<DB>(
    tx: &mut Transaction<'_, DB>,
    observation: &ProjectionObservation,
) -> Result<(), ProjectionProtocolError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
    for<'q> &'q [u8]: Encode<'q, DB> + Type<DB>,
{
    let scope = &observation.scope;
    let topology_hash = scope.topology().digest();
    let partition_hash = scope.projection_partition().digest();
    let key_hash = scope.key_digest();
    let mut builder = QueryBuilder::<DB>::new(
        "INSERT INTO projection_observations \
         (topology_hash, partition_hash, causation_id, model_name, scope_kind, \
         canonical_key_bytes, canonical_key_hash, incarnation, revision, \
         change_epoch, change_position) VALUES (",
    );
    builder.push_bind(topology_hash.as_slice());
    builder.push(", ");
    builder.push_bind(partition_hash.as_slice());
    builder.push(", ");
    builder.push_bind(observation.causation_id.as_str());
    builder.push(", ");
    builder.push_bind(scope.model());
    builder.push(", ");
    builder.push_bind(observation.kind.as_storage_str());
    builder.push(", ");
    builder.push_bind(scope.canonical_key_bytes());
    builder.push(", ");
    builder.push_bind(key_hash.as_slice());
    builder.push(", ");
    if let Some(revision) = &observation.revision {
        builder.push_bind(to_i64::<DB>(
            revision.incarnation(),
            "observation record incarnation",
        )?);
    } else {
        builder.push("NULL");
    }
    builder.push(", ");
    if let Some(revision) = &observation.revision {
        builder.push_bind(to_i64::<DB>(
            revision.revision(),
            "observation record revision",
        )?);
    } else {
        builder.push("NULL");
    }
    builder.push(", ");
    builder.push_bind(observation.change.epoch().as_str());
    builder.push(", ");
    builder.push_bind(to_i64::<DB>(
        observation.change.position(),
        "observation change position",
    )?);
    builder.push(")");
    builder
        .build()
        .execute(&mut **tx)
        .await
        .map_err(|error| protocol_storage_error::<DB>("insert projection observation", error))?;
    Ok(())
}

async fn store_input_cursor_in_tx<DB>(
    tx: &mut Transaction<'_, DB>,
    input: &TrustedProjectionInput,
    change: &ProjectionChangeCursor,
) -> Result<(), ProjectionProtocolError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
    for<'q> &'q [u8]: Encode<'q, DB> + Type<DB>,
{
    let cursor = &input.cursor;
    let topology_hash = cursor.topology().digest();
    let partition_hash = cursor.projection_partition().digest();
    let source = cursor.source();
    let source_bytes = source.canonical_name_bytes();
    let source_hash = source.digest();
    let source_partition_hash = source.partition_digest();
    let mut builder = QueryBuilder::<DB>::new(
        "INSERT INTO projection_input_cursors \
         (topology_hash, partition_hash, source_bytes, source_hash, source_partition_bytes, \
         source_partition_hash, source_epoch, source_position, input_hash, message_id, \
         causation_id, gap_free, generation, change_epoch, change_position) VALUES (",
    );
    builder.push_bind(topology_hash.as_slice());
    builder.push(", ");
    builder.push_bind(partition_hash.as_slice());
    builder.push(", ");
    builder.push_bind(source_bytes.as_slice());
    builder.push(", ");
    builder.push_bind(source_hash.as_slice());
    builder.push(", ");
    builder.push_bind(source.canonical_partition_bytes());
    builder.push(", ");
    builder.push_bind(source_partition_hash.as_slice());
    builder.push(", ");
    builder.push_bind(cursor.epoch().as_str());
    builder.push(", ");
    builder.push_bind(to_i64::<DB>(
        cursor.position(),
        "projection input position",
    )?);
    builder.push(", ");
    let input_hash = input.fingerprint.digest();
    builder.push_bind(input_hash.as_slice());
    builder.push(", ");
    builder.push_bind(input.message_id.as_str());
    builder.push(", ");
    builder.push_bind(input.causation_id.as_str());
    builder.push(", ");
    builder.push_bind(i64::from(input.gap_free));
    builder.push(", ");
    builder.push_bind(to_i64::<DB>(
        input.generation.get(),
        "projection generation",
    )?);
    builder.push(", ");
    builder.push_bind(change.epoch().as_str());
    builder.push(", ");
    builder.push_bind(to_i64::<DB>(
        change.position(),
        "projection change position",
    )?);
    builder.push(
        ") ON CONFLICT \
         (topology_hash, partition_hash, source_hash, source_partition_hash, generation) \
         DO UPDATE SET source_bytes = excluded.source_bytes, \
         source_partition_bytes = excluded.source_partition_bytes, source_epoch = excluded.source_epoch, \
         source_position = excluded.source_position, input_hash = excluded.input_hash, \
         message_id = excluded.message_id, causation_id = excluded.causation_id, \
         gap_free = excluded.gap_free, change_epoch = excluded.change_epoch, \
         change_position = excluded.change_position",
    );
    builder
        .build()
        .execute(&mut **tx)
        .await
        .map_err(|error| protocol_storage_error::<DB>("store projection input cursor", error))?;
    Ok(())
}

async fn insert_input_identity_in_tx<DB>(
    tx: &mut Transaction<'_, DB>,
    input: &TrustedProjectionInput,
) -> Result<(), ProjectionProtocolError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> String: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> Vec<u8>: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
    for<'q> &'q [u8]: Encode<'q, DB> + Type<DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    let cursor = &input.cursor;
    let topology_hash = cursor.topology().digest();
    let partition_hash = cursor.projection_partition().digest();
    let source = cursor.source();
    let source_bytes = source.canonical_name_bytes();
    let source_hash = source.digest();
    let source_partition_hash = source.partition_digest();
    let input_hash = input.fingerprint.digest();
    let mut builder = QueryBuilder::<DB>::new(
        "INSERT INTO projection_input_identities \
         (topology_hash, partition_hash, source_bytes, source_hash, source_partition_bytes, \
         source_partition_hash, source_epoch, source_position, input_hash, message_id, \
         causation_id, gap_free) VALUES (",
    );
    builder.push_bind(topology_hash.as_slice());
    builder.push(", ");
    builder.push_bind(partition_hash.as_slice());
    builder.push(", ");
    builder.push_bind(source_bytes.as_slice());
    builder.push(", ");
    builder.push_bind(source_hash.as_slice());
    builder.push(", ");
    builder.push_bind(source.canonical_partition_bytes());
    builder.push(", ");
    builder.push_bind(source_partition_hash.as_slice());
    builder.push(", ");
    builder.push_bind(cursor.epoch().as_str());
    builder.push(", ");
    builder.push_bind(to_i64::<DB>(
        cursor.position(),
        "projection input identity position",
    )?);
    builder.push(", ");
    builder.push_bind(input_hash.as_slice());
    builder.push(", ");
    builder.push_bind(input.message_id.as_str());
    builder.push(", ");
    builder.push_bind(input.causation_id.as_str());
    builder.push(", ");
    builder.push_bind(i64::from(input.gap_free));
    builder.push(") ON CONFLICT DO NOTHING");
    let result =
        builder.build().execute(&mut **tx).await.map_err(|error| {
            protocol_storage_error::<DB>("insert projection input identity", error)
        })?;
    if DB::rows_affected(&result) == 1 {
        return Ok(());
    }

    if let Some(existing) = input_identity_by_cursor_in_tx(tx, input).await? {
        if input_identity_matches(&existing, input) {
            return Ok(());
        }
        return Err(ProjectionProtocolError::InputCorruption);
    }
    if input_identity_by_message_in_tx(tx, input).await?.is_some() {
        return Err(ProjectionProtocolError::MessageIdReuse {
            message_id: input.message_id.clone(),
        });
    }
    Err(corrupt_storage(
        "projection input identity collided without a readable conflicting row",
    ))
}

async fn insert_input_receipt_in_tx<DB>(
    tx: &mut Transaction<'_, DB>,
    input: &TrustedProjectionInput,
    outcome_kind: &'static str,
    failure_id: Option<&str>,
    change: &ProjectionChangeCursor,
) -> Result<(), ProjectionProtocolError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
    for<'q> &'q [u8]: Encode<'q, DB> + Type<DB>,
{
    let cursor = &input.cursor;
    let topology_hash = cursor.topology().digest();
    let partition_hash = cursor.projection_partition().digest();
    let source = cursor.source();
    let source_bytes = source.canonical_name_bytes();
    let source_hash = source.digest();
    let source_partition_hash = source.partition_digest();
    let input_hash = input.fingerprint.digest();
    let mut builder = QueryBuilder::<DB>::new(
        "INSERT INTO projection_input_receipts \
         (topology_hash, partition_hash, generation, message_id, source_bytes, source_hash, \
         source_partition_bytes, source_partition_hash, source_epoch, source_position, input_hash, \
         causation_id, gap_free, outcome_kind, failure_id, change_epoch, change_position) VALUES (",
    );
    builder.push_bind(topology_hash.as_slice());
    builder.push(", ");
    builder.push_bind(partition_hash.as_slice());
    builder.push(", ");
    builder.push_bind(to_i64::<DB>(
        input.generation.get(),
        "projection generation",
    )?);
    builder.push(", ");
    builder.push_bind(input.message_id.as_str());
    builder.push(", ");
    builder.push_bind(source_bytes.as_slice());
    builder.push(", ");
    builder.push_bind(source_hash.as_slice());
    builder.push(", ");
    builder.push_bind(source.canonical_partition_bytes());
    builder.push(", ");
    builder.push_bind(source_partition_hash.as_slice());
    builder.push(", ");
    builder.push_bind(cursor.epoch().as_str());
    builder.push(", ");
    builder.push_bind(to_i64::<DB>(
        cursor.position(),
        "projection input position",
    )?);
    builder.push(", ");
    builder.push_bind(input_hash.as_slice());
    builder.push(", ");
    builder.push_bind(input.causation_id.as_str());
    builder.push(", ");
    builder.push_bind(i64::from(input.gap_free));
    builder.push(", ");
    builder.push_bind(outcome_kind);
    builder.push(", ");
    if let Some(failure_id) = failure_id {
        builder.push_bind(failure_id);
    } else {
        builder.push("NULL");
    }
    builder.push(", ");
    builder.push_bind(change.epoch().as_str());
    builder.push(", ");
    builder.push_bind(to_i64::<DB>(
        change.position(),
        "projection change position",
    )?);
    builder.push(") ON CONFLICT DO NOTHING");
    let result =
        builder.build().execute(&mut **tx).await.map_err(|error| {
            protocol_storage_error::<DB>("insert projection input receipt", error)
        })?;
    if DB::rows_affected(&result) != 1 {
        return Err(corrupt_storage(
            "projection input receipt collided while its partition lock was held",
        ));
    }
    Ok(())
}

async fn ensure_inbox_available_in_tx<DB>(
    tx: &mut Transaction<'_, DB>,
    input: &TrustedProjectionInput,
) -> Result<(), ProjectionProtocolError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
{
    let receipt = input.inbox_receipt();
    receipt.validate()?;
    let mut builder = QueryBuilder::<DB>::new("SELECT 1 FROM consumer_inbox WHERE consumer = ");
    builder.push_bind(receipt.consumer.as_str());
    builder.push(" AND message_id = ");
    builder.push_bind(receipt.message_id.as_str());
    builder.push(" LIMIT 1");
    if builder
        .build()
        .fetch_optional(&mut **tx)
        .await
        .map_err(|error| protocol_storage_error::<DB>("check projection consumer inbox", error))?
        .is_some()
    {
        return Err(ProjectionProtocolError::MessageIdReuse {
            message_id: input.message_id.clone(),
        });
    }
    Ok(())
}

async fn insert_inbox_in_tx<DB>(
    tx: &mut Transaction<'_, DB>,
    input: &TrustedProjectionInput,
) -> Result<(), ProjectionProtocolError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
{
    let receipt = input.inbox_receipt();
    let mut builder =
        QueryBuilder::<DB>::new("INSERT INTO consumer_inbox (consumer, message_id) VALUES (");
    builder.push_bind(receipt.consumer.as_str());
    builder.push(", ");
    builder.push_bind(receipt.message_id.as_str());
    builder.push(") ON CONFLICT DO NOTHING");
    let result =
        builder.build().execute(&mut **tx).await.map_err(|error| {
            protocol_storage_error::<DB>("insert projection consumer inbox", error)
        })?;
    if DB::rows_affected(&result) != 1 {
        return Err(corrupt_storage(
            "projection consumer inbox collided while its partition lock was held",
        ));
    }
    Ok(())
}

async fn update_partition_head_in_tx<DB>(
    tx: &mut Transaction<'_, DB>,
    topology: &ProjectorTopologyId,
    partition: &ProjectionPartition,
    change_head: u64,
    clear_pending_retry: Option<&str>,
) -> Result<(), ProjectionProtocolError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
    for<'q> &'q [u8]: Encode<'q, DB> + Type<DB>,
{
    let topology_hash = topology.digest();
    let partition_hash = partition.digest();
    let mut builder = QueryBuilder::<DB>::new("UPDATE projection_partitions SET change_head = ");
    builder.push_bind(to_i64::<DB>(change_head, "projection change head")?);
    if clear_pending_retry.is_some() {
        builder.push(", pending_retry_failure_id = NULL");
    }
    builder.push(" WHERE topology_hash = ");
    builder.push_bind(topology_hash.as_slice());
    builder.push(" AND partition_hash = ");
    builder.push_bind(partition_hash.as_slice());
    if let Some(failure_id) = clear_pending_retry {
        builder.push(" AND pending_retry_failure_id = ");
        builder.push_bind(failure_id);
    }
    let result =
        builder.build().execute(&mut **tx).await.map_err(|error| {
            protocol_storage_error::<DB>("advance projection change head", error)
        })?;
    if DB::rows_affected(&result) != 1 {
        return Err(corrupt_storage(
            "projection partition or pending retry fence changed while its lock was held",
        ));
    }
    Ok(())
}

async fn retain_projection_change_suffix_in_tx<DB>(
    tx: &mut Transaction<'_, DB>,
    topology: &ProjectorTopologyId,
    partition: &ProjectionPartition,
    state: &PartitionState,
    retention: ProjectionChangeRetention,
) -> Result<u64, ProjectionProtocolError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
    for<'q> &'q [u8]: Encode<'q, DB> + Type<DB>,
{
    let target = state.compacted_through.max(
        state
            .change_head
            .saturating_sub(retention.max_retained_changes()),
    );
    if target <= state.compacted_through {
        return Ok(state.compacted_through);
    }

    let topology_hash = topology.digest();
    let partition_hash = partition.digest();
    let old_watermark = to_i64::<DB>(state.compacted_through, "projection compaction watermark")?;
    let target_watermark = to_i64::<DB>(target, "projection compaction watermark")?;
    let mut delete =
        QueryBuilder::<DB>::new("DELETE FROM projection_changes WHERE topology_hash = ");
    delete.push_bind(topology_hash.as_slice());
    delete.push(" AND partition_hash = ");
    delete.push_bind(partition_hash.as_slice());
    delete.push(" AND change_epoch = ");
    delete.push_bind(state.change_epoch.as_str());
    delete.push(" AND change_position > ");
    delete.push_bind(old_watermark);
    delete.push(" AND change_position <= ");
    delete.push_bind(target_watermark);
    let result =
        delete.build().execute(&mut **tx).await.map_err(|error| {
            protocol_storage_error::<DB>("retain projection change suffix", error)
        })?;
    let expected_removed = target - state.compacted_through;
    if DB::rows_affected(&result) != expected_removed {
        return Err(corrupt_storage(format!(
            "projection retention expected to remove {expected_removed} changes but removed {}",
            DB::rows_affected(&result)
        )));
    }

    let mut update =
        QueryBuilder::<DB>::new("UPDATE projection_partitions SET compacted_through = ");
    update.push_bind(target_watermark);
    update.push(" WHERE topology_hash = ");
    update.push_bind(topology_hash.as_slice());
    update.push(" AND partition_hash = ");
    update.push_bind(partition_hash.as_slice());
    update.push(" AND change_epoch = ");
    update.push_bind(state.change_epoch.as_str());
    update.push(" AND compacted_through = ");
    update.push_bind(old_watermark);
    update.push(" AND change_head = ");
    update.push_bind(to_i64::<DB>(state.change_head, "projection change head")?);
    let result = update.build().execute(&mut **tx).await.map_err(|error| {
        protocol_storage_error::<DB>("advance projection retention watermark", error)
    })?;
    if DB::rows_affected(&result) != 1 {
        return Err(corrupt_storage(
            "projection partition changed while retaining its change suffix",
        ));
    }
    Ok(target)
}

fn decode_failure_row<DB>(
    row: &DB::Row,
    topology: &ProjectorTopologyId,
    partition: &ProjectionPartition,
    expected_change_epoch: &ProjectionEpoch,
) -> Result<StoredFailure, ProjectionProtocolError>
where
    DB: SqlxRepoBackend,
    for<'q> i64: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> String: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> Vec<u8>: Type<DB> + sqlx::Decode<'q, DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    let source_bytes: Vec<u8> = row
        .try_get("source_bytes")
        .map_err(|error| protocol_storage_error::<DB>("decode failure source bytes", error))?;
    let source_hash: Vec<u8> = row
        .try_get("source_hash")
        .map_err(|error| protocol_storage_error::<DB>("decode failure source hash", error))?;
    let source_partition_bytes: Vec<u8> =
        row.try_get("source_partition_bytes").map_err(|error| {
            protocol_storage_error::<DB>("decode failure source partition bytes", error)
        })?;
    let source_partition_hash: Vec<u8> = row.try_get("source_partition_hash").map_err(|error| {
        protocol_storage_error::<DB>("decode failure source partition hash", error)
    })?;
    let source =
        ProjectionSource::from_canonical_name_bytes(&source_bytes, source_partition_bytes.clone())?;
    verify_digest(&source_hash, source.digest(), "projection failure source")?;
    verify_digest(
        &source_partition_hash,
        source.partition_digest(),
        "projection failure source partition",
    )?;
    let source_epoch: String = row
        .try_get("source_epoch")
        .map_err(|error| protocol_storage_error::<DB>("decode failure source epoch", error))?;
    let source_position = from_i64::<DB>(
        row.try_get("source_position").map_err(|error| {
            protocol_storage_error::<DB>("decode failure source position", error)
        })?,
        "failure source position",
    )?;
    let input_hash = decode_digest(
        row.try_get("input_hash")
            .map_err(|error| protocol_storage_error::<DB>("decode failure input hash", error))?,
        "projection input",
    )?;
    let gap_free = match row
        .try_get::<i64, _>("gap_free")
        .map_err(|error| protocol_storage_error::<DB>("decode failure gap-free flag", error))?
    {
        0 => false,
        1 => true,
        value => {
            return Err(corrupt_storage(format!(
                "failure gap-free flag contains invalid value {value}"
            )))
        }
    };
    let failure_bytes: Vec<u8> = row
        .try_get("failure_bytes")
        .map_err(|error| protocol_storage_error::<DB>("decode projection failure bytes", error))?;
    let failure_digest = decode_digest(
        row.try_get("failure_hash").map_err(|error| {
            protocol_storage_error::<DB>("decode projection failure hash", error)
        })?,
        "projection failure",
    )?;
    if ProjectionFailureBatch::fingerprint_bytes(&failure_bytes) != failure_digest {
        return Err(corrupt_storage(
            "projection failure bytes do not match their stored digest",
        ));
    }
    let change_epoch: String = row
        .try_get("change_epoch")
        .map_err(|error| protocol_storage_error::<DB>("decode failure change epoch", error))?;
    let change_epoch = ProjectionEpoch::new(change_epoch)?;
    if &change_epoch != expected_change_epoch {
        return Err(corrupt_storage(
            "projection failure change epoch differs from its partition",
        ));
    }
    let change_position = from_i64::<DB>(
        row.try_get("change_position").map_err(|error| {
            protocol_storage_error::<DB>("decode failure change position", error)
        })?,
        "failure change position",
    )?;
    let generation = ProjectionGeneration::new(from_i64::<DB>(
        row.try_get("generation")
            .map_err(|error| protocol_storage_error::<DB>("decode failure generation", error))?,
        "projection failure generation",
    )?)?;
    Ok(StoredFailure {
        failure: ProjectionFailure {
            failure_id: row.try_get("failure_id").map_err(|error| {
                protocol_storage_error::<DB>("decode projection failure ID", error)
            })?,
            input: ProjectionInputCursor::new(
                topology.clone(),
                partition.clone(),
                source,
                ProjectionEpoch::new(source_epoch)?,
                source_position,
            )?,
            input_fingerprint: ProjectionInputFingerprint::from_digest(input_hash),
            message_id: row.try_get("message_id").map_err(|error| {
                protocol_storage_error::<DB>("decode failure message ID", error)
            })?,
            causation_id: row.try_get("causation_id").map_err(|error| {
                protocol_storage_error::<DB>("decode failure causation ID", error)
            })?,
            generation,
            gap_free,
            failure_code: row.try_get("failure_code").map_err(|error| {
                protocol_storage_error::<DB>("decode projection failure code", error)
            })?,
            failure_bytes,
            failure_digest,
            change: ProjectionChangeCursor::new(
                topology.clone(),
                partition.clone(),
                change_epoch,
                change_position,
            )?,
        },
    })
}

async fn failure_in_tx<DB>(
    tx: &mut Transaction<'_, DB>,
    topology: &ProjectorTopologyId,
    partition: &ProjectionPartition,
    failure_id: &str,
    expected_change_epoch: &ProjectionEpoch,
) -> Result<Option<StoredFailure>, ProjectionProtocolError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> String: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> Vec<u8>: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
    for<'q> &'q [u8]: Encode<'q, DB> + Type<DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    let topology_hash = topology.digest();
    let partition_hash = partition.digest();
    let mut builder = QueryBuilder::<DB>::new(
        "SELECT failure_id, source_bytes, source_hash, source_partition_bytes, \
         source_partition_hash, source_epoch, source_position, input_hash, message_id, \
         causation_id, gap_free, generation, failure_code, failure_bytes, failure_hash, \
         change_epoch, change_position FROM projection_failures WHERE topology_hash = ",
    );
    builder.push_bind(topology_hash.as_slice());
    builder.push(" AND partition_hash = ");
    builder.push_bind(partition_hash.as_slice());
    builder.push(" AND failure_id = ");
    builder.push_bind(failure_id);
    let row = builder
        .build()
        .fetch_optional(&mut **tx)
        .await
        .map_err(|error| protocol_storage_error::<DB>("load projection failure", error))?;
    row.map(|row| decode_failure_row::<DB>(&row, topology, partition, expected_change_epoch))
        .transpose()
}

fn failure_matches_batch(failure: &StoredFailure, batch: &ProjectionFailureBatch) -> bool {
    failure.failure.input == batch.input.cursor
        && failure.failure.input_fingerprint == batch.input.fingerprint
        && failure.failure.message_id == batch.input.message_id
        && failure.failure.causation_id == batch.input.causation_id
        && failure.failure.gap_free == batch.input.gap_free
        && failure.failure.generation == batch.input.generation
        && failure.failure.failure_code == batch.failure_code
        && failure.failure.failure_bytes == batch.failure_bytes
        && failure.failure.failure_digest == batch.failure_digest
        && failure.failure.change.epoch() == &batch.change_epoch
}

async fn ensure_pending_retry_input_in_tx<DB>(
    tx: &mut Transaction<'_, DB>,
    state: &PartitionState,
    input: &TrustedProjectionInput,
) -> Result<(), ProjectionProtocolError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> String: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> Vec<u8>: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
    for<'q> &'q [u8]: Encode<'q, DB> + Type<DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    let Some(failure_id) = &state.pending_retry_failure_id else {
        return Ok(());
    };
    let failure = failure_in_tx(
        tx,
        input.cursor.topology(),
        input.cursor.projection_partition(),
        failure_id,
        &state.change_epoch,
    )
    .await?
    .ok_or_else(|| {
        corrupt_storage(format!(
            "pending projection retry failure `{failure_id}` is missing"
        ))
    })?;
    if failure.failure.generation.checked_next()? != state.active_generation {
        return Err(corrupt_storage(format!(
            "pending retry failure generation {} does not precede active generation {}",
            failure.failure.generation.get(),
            state.active_generation.get()
        )));
    }
    if failure.failure.input != input.cursor {
        return Err(ProjectionProtocolError::IncomparableInput);
    }
    if failure.failure.input_fingerprint != input.fingerprint
        || failure.failure.message_id != input.message_id
        || failure.failure.causation_id != input.causation_id
        || failure.failure.gap_free != input.gap_free
    {
        return Err(ProjectionProtocolError::InputCorruption);
    }
    Ok(())
}

async fn insert_failure_in_tx<DB>(
    tx: &mut Transaction<'_, DB>,
    batch: &ProjectionFailureBatch,
    change: &ProjectionChangeCursor,
) -> Result<(), ProjectionProtocolError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
    for<'q> &'q [u8]: Encode<'q, DB> + Type<DB>,
{
    let cursor = &batch.input.cursor;
    let topology_hash = cursor.topology().digest();
    let partition_hash = cursor.projection_partition().digest();
    let source = cursor.source();
    let source_bytes = source.canonical_name_bytes();
    let source_hash = source.digest();
    let source_partition_hash = source.partition_digest();
    let input_hash = batch.input.fingerprint.digest();
    let mut builder = QueryBuilder::<DB>::new(
        "INSERT INTO projection_failures \
         (topology_hash, partition_hash, failure_id, source_bytes, source_hash, \
         source_partition_bytes, source_partition_hash, source_epoch, source_position, \
         input_hash, message_id, causation_id, gap_free, generation, failure_code, \
         failure_bytes, failure_hash, change_epoch, change_position) VALUES (",
    );
    builder.push_bind(topology_hash.as_slice());
    builder.push(", ");
    builder.push_bind(partition_hash.as_slice());
    builder.push(", ");
    builder.push_bind(batch.failure_id.as_str());
    builder.push(", ");
    builder.push_bind(source_bytes.as_slice());
    builder.push(", ");
    builder.push_bind(source_hash.as_slice());
    builder.push(", ");
    builder.push_bind(source.canonical_partition_bytes());
    builder.push(", ");
    builder.push_bind(source_partition_hash.as_slice());
    builder.push(", ");
    builder.push_bind(cursor.epoch().as_str());
    builder.push(", ");
    builder.push_bind(to_i64::<DB>(
        cursor.position(),
        "projection failure source position",
    )?);
    builder.push(", ");
    builder.push_bind(input_hash.as_slice());
    builder.push(", ");
    builder.push_bind(batch.input.message_id.as_str());
    builder.push(", ");
    builder.push_bind(batch.input.causation_id.as_str());
    builder.push(", ");
    builder.push_bind(i64::from(batch.input.gap_free));
    builder.push(", ");
    builder.push_bind(to_i64::<DB>(
        batch.input.generation.get(),
        "projection failure generation",
    )?);
    builder.push(", ");
    builder.push_bind(batch.failure_code.as_str());
    builder.push(", ");
    builder.push_bind(batch.failure_bytes.as_slice());
    builder.push(", ");
    builder.push_bind(batch.failure_digest.as_slice());
    builder.push(", ");
    builder.push_bind(change.epoch().as_str());
    builder.push(", ");
    builder.push_bind(to_i64::<DB>(
        change.position(),
        "projection failure change position",
    )?);
    builder.push(") ON CONFLICT DO NOTHING");
    let result = builder
        .build()
        .execute(&mut **tx)
        .await
        .map_err(|error| protocol_storage_error::<DB>("insert projection failure", error))?;
    if DB::rows_affected(&result) != 1 {
        return Err(corrupt_storage(
            "projection failure collided while its partition lock was held",
        ));
    }
    Ok(())
}

async fn stop_partition_in_tx<DB>(
    tx: &mut Transaction<'_, DB>,
    batch: &ProjectionFailureBatch,
    change_head: u64,
) -> Result<(), ProjectionProtocolError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
    for<'q> &'q [u8]: Encode<'q, DB> + Type<DB>,
{
    let cursor = &batch.input.cursor;
    let topology_hash = cursor.topology().digest();
    let partition_hash = cursor.projection_partition().digest();
    let source = cursor.source();
    let source_bytes = source.canonical_name_bytes();
    let source_hash = source.digest();
    let source_partition_hash = source.partition_digest();
    let input_hash = batch.input.fingerprint.digest();
    let mut builder = QueryBuilder::<DB>::new("UPDATE projection_partitions SET change_head = ");
    builder.push_bind(to_i64::<DB>(change_head, "projection change head")?);
    builder.push(", pending_retry_failure_id = NULL, stopped_failure_id = ");
    builder.push_bind(batch.failure_id.as_str());
    builder.push(", stopped_source_bytes = ");
    builder.push_bind(source_bytes.as_slice());
    builder.push(", stopped_source_hash = ");
    builder.push_bind(source_hash.as_slice());
    builder.push(", stopped_source_partition_bytes = ");
    builder.push_bind(source.canonical_partition_bytes());
    builder.push(", stopped_source_partition_hash = ");
    builder.push_bind(source_partition_hash.as_slice());
    builder.push(", stopped_source_epoch = ");
    builder.push_bind(cursor.epoch().as_str());
    builder.push(", stopped_source_position = ");
    builder.push_bind(to_i64::<DB>(
        cursor.position(),
        "stopped projection source position",
    )?);
    builder.push(", stopped_generation = ");
    builder.push_bind(to_i64::<DB>(
        batch.input.generation.get(),
        "stopped projection generation",
    )?);
    builder.push(", stopped_input_hash = ");
    builder.push_bind(input_hash.as_slice());
    builder.push(", stopped_message_id = ");
    builder.push_bind(batch.input.message_id.as_str());
    builder.push(", stopped_causation_id = ");
    builder.push_bind(batch.input.causation_id.as_str());
    builder.push(", stopped_gap_free = ");
    builder.push_bind(i64::from(batch.input.gap_free));
    builder.push(" WHERE topology_hash = ");
    builder.push_bind(topology_hash.as_slice());
    builder.push(" AND partition_hash = ");
    builder.push_bind(partition_hash.as_slice());
    let result = builder
        .build()
        .execute(&mut **tx)
        .await
        .map_err(|error| protocol_storage_error::<DB>("stop projection partition", error))?;
    if DB::rows_affected(&result) != 1 {
        return Err(corrupt_storage(
            "projection partition disappeared while recording its failure",
        ));
    }
    Ok(())
}

fn decode_change_row<DB>(
    row: &DB::Row,
    topology: &ProjectorTopologyId,
    partition: &ProjectionPartition,
    expected_epoch: &ProjectionEpoch,
) -> Result<ProjectionChange, ProjectionProtocolError>
where
    DB: SqlxRepoBackend,
    for<'q> i64: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> String: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> Vec<u8>: Type<DB> + sqlx::Decode<'q, DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    let change_epoch: String = row
        .try_get("change_epoch")
        .map_err(|error| protocol_storage_error::<DB>("decode projection change epoch", error))?;
    let change_epoch = ProjectionEpoch::new(change_epoch)?;
    if &change_epoch != expected_epoch {
        return Err(corrupt_storage(
            "projection change epoch differs from its partition",
        ));
    }
    let change_position = from_i64::<DB>(
        row.try_get("change_position").map_err(|error| {
            protocol_storage_error::<DB>("decode projection change position", error)
        })?,
        "projection change position",
    )?;
    let kind_value: String = row
        .try_get("change_kind")
        .map_err(|error| protocol_storage_error::<DB>("decode projection change kind", error))?;
    let kind = decode_change_kind(&kind_value)?;
    let causation_id: String = row
        .try_get("causation_id")
        .map_err(|error| protocol_storage_error::<DB>("decode change causation ID", error))?;
    let model_name: Option<String> = row
        .try_get("model_name")
        .map_err(|error| protocol_storage_error::<DB>("decode change model", error))?;
    let scope_kind: Option<String> = row
        .try_get("scope_kind")
        .map_err(|error| protocol_storage_error::<DB>("decode change scope kind", error))?;
    let key_bytes: Option<Vec<u8>> = row
        .try_get("canonical_key_bytes")
        .map_err(|error| protocol_storage_error::<DB>("decode change key bytes", error))?;
    let key_hash: Option<Vec<u8>> = row
        .try_get("canonical_key_hash")
        .map_err(|error| protocol_storage_error::<DB>("decode change key hash", error))?;
    let incarnation: Option<i64> = row
        .try_get("incarnation")
        .map_err(|error| protocol_storage_error::<DB>("decode change record incarnation", error))?;
    let revision_value: Option<i64> = row
        .try_get("revision")
        .map_err(|error| protocol_storage_error::<DB>("decode change record revision", error))?;
    let failure_id: Option<String> = row
        .try_get("failure_id")
        .map_err(|error| protocol_storage_error::<DB>("decode change failure ID", error))?;

    let scope = match (model_name, key_bytes, key_hash) {
        (Some(model), Some(bytes), Some(hash)) => {
            let scope =
                ProjectionRecordScope::new(topology.clone(), partition.clone(), model, bytes)?;
            verify_digest(&hash, scope.key_digest(), "projection change key")?;
            Some(scope)
        }
        (None, None, None) => None,
        _ => {
            return Err(corrupt_storage(
                "projection change record scope has an inconsistent shape",
            ))
        }
    };
    let revision = match (scope.as_ref(), incarnation, revision_value) {
        (Some(scope), Some(incarnation), Some(revision)) => Some(RecordRevision::new(
            scope.clone(),
            from_i64::<DB>(incarnation, "change record incarnation")?,
            from_i64::<DB>(revision, "change record revision")?,
        )?),
        (_, None, None) => None,
        _ => {
            return Err(corrupt_storage(
                "projection change record revision has an inconsistent shape",
            ))
        }
    };
    let observation_kind = scope_kind
        .as_deref()
        .map(decode_observation_kind)
        .transpose()?;
    match kind {
        ProjectionChangeKind::RecordUpsert
        | ProjectionChangeKind::RecordDelete
        | ProjectionChangeKind::RecordRecreate
            if scope.is_some()
                && revision.is_some()
                && observation_kind.is_none()
                && failure_id.is_none() => {}
        ProjectionChangeKind::Observation
            if scope.is_some()
                && observation_kind.is_some()
                && failure_id.is_none()
                && ((observation_kind == Some(ProjectionObservationKind::Record)
                    && revision.is_some())
                    || (observation_kind == Some(ProjectionObservationKind::Dependency)
                        && revision.is_none())) => {}
        ProjectionChangeKind::Checkpoint
            if scope.is_none()
                && revision.is_none()
                && observation_kind.is_none()
                && failure_id.is_none() => {}
        ProjectionChangeKind::Failure
            if scope.is_none()
                && revision.is_none()
                && observation_kind.is_none()
                && failure_id.is_some() => {}
        _ => {
            return Err(corrupt_storage(format!(
                "projection change `{kind_value}` payload has an inconsistent shape"
            )))
        }
    }
    Ok(ProjectionChange {
        cursor: ProjectionChangeCursor::new(
            topology.clone(),
            partition.clone(),
            change_epoch,
            change_position,
        )?,
        kind,
        causation_id,
        observation_kind,
        scope,
        revision,
        failure_id,
    })
}

/// Apply one sealed same-transaction command projection inside its caller's
/// already-open domain/ledger transaction.
///
/// The adapter, rather than the command handler, allocates record revisions
/// and change cursors while holding the authoritative projection partition
/// lock. The returned evidence is attached to the command completion before
/// the caller executes the final ledger fence.
pub(crate) async fn apply_same_transaction_projection_in_tx<DB>(
    tx: &mut Transaction<'_, DB>,
    batch: &SameTransactionProjectionBatch,
    retention: ProjectionChangeRetention,
) -> Result<SameTransactionProjectionEvidence, ProjectionProtocolError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> String: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> Vec<u8>: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
    for<'q> &'q [u8]: Encode<'q, DB> + Type<DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    batch.validate()?;
    let write_plan = TableWritePlan::new(
        batch
            .mutations
            .iter()
            .map(|mutation| mutation.mutation.clone())
            .collect(),
    );
    validate_sql_write_plan(&write_plan)?;
    verify_registered_topology_in_tx(tx, &batch.topology).await?;
    let mut state =
        lock_partition_in_tx(tx, &batch.topology, &batch.partition, &batch.change_epoch).await?;
    if let Some(failure_id) = &state.stopped_failure_id {
        return Err(ProjectionProtocolError::PartitionStopped {
            failure_id: failure_id.clone(),
        });
    }
    if state.pending_retry_failure_id.is_some() {
        return Err(ProjectionProtocolError::IncomparableInput);
    }
    ensure_partition_ownership_in_tx(tx, &batch.topology, &batch.partition, &batch.ownership)
        .await?;

    let staged = batch
        .mutations
        .first()
        .expect("same-transaction projection validation requires one mutation");
    let owner = batch
        .ownership
        .first()
        .expect("same-transaction projection validation requires one owner");
    if staged.mutation.table_name() != owner.table
        || table_model_name(&staged.mutation) != staged.scope.model()
    {
        return Err(ProjectionProtocolError::InvalidBatch(format!(
            "direct projection mutation for model `{}` does not target its registered table",
            staged.scope.model()
        )));
    }

    let current = record_in_tx(tx, &staged.scope, &state.change_epoch).await?;
    let physical_exists = physical_row_exists_in_tx(tx, &staged.mutation).await?;
    match current.as_ref().map(|record| &record.metadata) {
        None if physical_exists => {
            return Err(ProjectionProtocolError::RecordAlreadyExists {
                model: staged.scope.model().to_string(),
            });
        }
        Some(metadata) if metadata.tombstone => {
            return Err(ProjectionProtocolError::RecordTombstoned {
                model: staged.scope.model().to_string(),
            });
        }
        Some(_) if !physical_exists => {
            return Err(ProjectionProtocolError::RecordMissing {
                model: staged.scope.model().to_string(),
            });
        }
        _ => {}
    }
    let expectation = current
        .as_ref()
        .map(|record| ProjectionRecordExpectation::Exact(record.metadata.revision.clone()))
        .unwrap_or(ProjectionRecordExpectation::Missing);
    let (revision, tombstone) = next_record(
        &staged.scope,
        &expectation,
        ProjectionMutationKind::Upsert,
        current.as_ref(),
    )?;
    debug_assert!(!tombstone);
    let change = allocate_change(
        &mut state,
        &batch.topology,
        &batch.partition,
        ProjectionChangeKind::RecordUpsert,
        batch.causation_id.clone(),
        None,
        Some(staged.scope.clone()),
        Some(revision.clone()),
        None,
    )?;
    let metadata = ProjectionRecordMetadata {
        revision: revision.clone(),
        tombstone,
        change: change.cursor.clone(),
    };

    let observation_request = batch
        .observations
        .first()
        .expect("same-transaction projection validation requires one observation");
    if observation_in_tx(
        tx,
        &batch.causation_id,
        &staged.scope,
        observation_request.kind,
        &state.change_epoch,
    )
    .await?
    .is_some()
    {
        return Err(ProjectionProtocolError::InvalidBatch(format!(
            "direct projection causation `{}` already has record evidence",
            batch.causation_id
        )));
    }
    let observation = ProjectionObservation {
        causation_id: batch.causation_id.clone(),
        kind: observation_request.kind,
        scope: staged.scope.clone(),
        revision: Some(revision),
        change: change.cursor.clone(),
    };

    apply_read_model_write_plan_in_tx(tx, write_plan).await?;
    insert_change_in_tx(tx, &change).await?;
    upsert_record_in_tx(tx, &metadata).await?;
    insert_observation_in_tx(tx, &observation).await?;
    update_partition_head_in_tx(
        tx,
        &batch.topology,
        &batch.partition,
        state.change_head,
        None,
    )
    .await?;
    retain_projection_change_suffix_in_tx(tx, &batch.topology, &batch.partition, &state, retention)
        .await?;

    Ok(SameTransactionProjectionEvidence {
        records: vec![metadata],
        changes: vec![change],
        observations: vec![observation],
    })
}

/// Execute the entire query-row/evidence read as one SQL statement.
///
/// PostgreSQL's default transaction isolation takes a fresh snapshot for every
/// statement, so merely wrapping independent row and metadata selects in a
/// transaction would still permit mixed evidence. This joined statement gives
/// SQLite and PostgreSQL the same statement-level snapshot and repeats the
/// physical/protocol columns only when multiple explicit checkpoint probes
/// match.
pub(crate) async fn read_projection_query_snapshot_in_executor<'e, DB, E>(
    executor: E,
    request: &ProjectionQuerySnapshotRequest,
) -> Result<ProjectionQuerySnapshot, ProjectionProtocolError>
where
    DB: SqlxRepoBackend,
    E: Executor<'e, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> String: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> Vec<u8>: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
    for<'q> &'q [u8]: Encode<'q, DB> + Type<DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    request.validate()?;
    let schema = request.schema;
    let physical_version_column = version_column(schema)?;
    let topology_hash = request.scope.topology().digest();
    let partition_hash = request.scope.projection_partition().digest();
    let key_hash = request.scope.key_digest();

    let mut builder = QueryBuilder::<DB>::new("SELECT ");
    for (index, column) in schema.columns.iter().enumerate() {
        if index > 0 {
            builder.push(", ");
        }
        builder.push("snapshot_row.");
        builder.push(quote_identifier(&column.column_name));
        builder.push(" AS ");
        builder.push(quote_identifier(&format!("ps_row_{index}")));
    }
    if !schema.columns.is_empty() {
        builder.push(", ");
    }
    builder.push("snapshot_row.");
    builder.push(quote_identifier(physical_version_column));
    builder.push(
        " AS ps_row_version, \
         registered.topology_bytes AS ps_registered_topology_bytes, \
         registered.table_name AS ps_registered_table, \
         partition.topology_bytes AS ps_partition_topology_bytes, \
         partition.partition_bytes AS ps_partition_bytes, \
         partition.active_generation AS ps_active_generation, \
         partition.change_epoch AS ps_change_epoch, \
         partition.change_head AS ps_change_head, \
         partition.compacted_through AS ps_compacted, \
         record.canonical_key_bytes AS ps_record_key_bytes, \
         record.canonical_key_hash AS ps_record_key_hash, \
         record.incarnation AS ps_record_incarnation, \
         record.revision AS ps_record_revision, \
         record.tombstone AS ps_record_tombstone, \
         record.change_epoch AS ps_record_change_epoch, \
         record.change_position AS ps_record_change_position, \
         checkpoint.source_bytes AS ps_cursor_source_bytes, \
         checkpoint.source_hash AS ps_cursor_source_hash, \
         checkpoint.source_partition_bytes AS ps_cursor_partition_bytes, \
         checkpoint.source_partition_hash AS ps_cursor_partition_hash, \
         checkpoint.source_epoch AS ps_cursor_source_epoch, \
         checkpoint.source_position AS ps_cursor_source_position, \
         checkpoint.gap_free AS ps_cursor_gap_free, \
         checkpoint.generation AS ps_cursor_generation, \
         checkpoint.change_epoch AS ps_cursor_change_epoch, \
         checkpoint.change_position AS ps_cursor_change_position \
         FROM (SELECT 1 AS snapshot_anchor) AS snapshot_anchor \
         LEFT JOIN (SELECT ",
    );
    for (index, column) in schema.columns.iter().enumerate() {
        if index > 0 {
            builder.push(", ");
        }
        DB::push_select_column(&mut builder, column);
    }
    if !schema.columns.is_empty() {
        builder.push(", ");
    }
    builder.push(quote_identifier(physical_version_column));
    builder.push(" FROM ");
    builder.push(quote_identifier(&schema.table_name));
    push_key_predicates(&mut builder, schema, &request.key)?;
    builder.push(") AS snapshot_row ON 1 = 1 ");

    builder.push(
        "LEFT JOIN projection_registered_models AS registered \
         ON registered.topology_hash = ",
    );
    builder.push_bind(topology_hash.as_slice());
    builder.push(" AND registered.model_name = ");
    builder.push_bind(request.scope.model());

    builder.push(
        " LEFT JOIN projection_partitions AS partition \
         ON partition.topology_hash = ",
    );
    builder.push_bind(topology_hash.as_slice());
    builder.push(" AND partition.partition_hash = ");
    builder.push_bind(partition_hash.as_slice());

    builder.push(
        " LEFT JOIN projection_records AS record \
         ON record.topology_hash = partition.topology_hash \
         AND record.partition_hash = partition.partition_hash \
         AND record.model_name = ",
    );
    builder.push_bind(request.scope.model());
    builder.push(" AND record.canonical_key_hash = ");
    builder.push_bind(key_hash.as_slice());

    builder.push(
        " LEFT JOIN projection_input_cursors AS checkpoint \
         ON checkpoint.topology_hash = partition.topology_hash \
         AND checkpoint.partition_hash = partition.partition_hash AND ",
    );
    if request.checkpoint_probes.is_empty() {
        builder.push("1 = 0");
    } else {
        builder.push("(");
        for (index, probe) in request.checkpoint_probes.iter().enumerate() {
            if index > 0 {
                builder.push(" OR ");
            }
            builder.push("(checkpoint.source_hash = ");
            let source_hash = probe.source.digest();
            builder.push_bind(source_hash.as_slice());
            builder.push(" AND checkpoint.source_partition_hash = ");
            let source_partition_hash = probe.source.partition_digest();
            builder.push_bind(source_partition_hash.as_slice());
            builder.push(" AND checkpoint.generation = ");
            builder.push_bind(to_i64::<DB>(
                probe.generation.get(),
                "projection generation",
            )?);
            builder.push(")");
        }
        builder.push(")");
    }
    builder.push(
        " ORDER BY checkpoint.source_hash, checkpoint.source_partition_hash, \
         checkpoint.generation",
    );

    let rows = builder.build().fetch_all(executor).await.map_err(|error| {
        protocol_storage_error::<DB>("read atomic projection query snapshot", error)
    })?;
    let first = rows.first().ok_or_else(|| {
        corrupt_storage("atomic projection query snapshot returned no anchor row")
    })?;

    let row_version: Option<i64> = first.try_get("ps_row_version").map_err(|error| {
        protocol_storage_error::<DB>("decode projection query row presence", error)
    })?;
    let physical_row = if row_version.is_some() {
        let mut values = RowValues::new();
        for (index, column) in schema.columns.iter().enumerate() {
            let mut aliased = column.clone();
            aliased.column_name = format!("ps_row_{index}");
            values.insert(column.column_name.clone(), DB::row_value(first, &aliased)?);
        }
        validate_row_values(schema, &values, true)?;
        validate_values_match_key(schema, &request.key, &values)?;
        Some(values)
    } else {
        None
    };

    let registered_topology: Option<Vec<u8>> = first
        .try_get("ps_registered_topology_bytes")
        .map_err(|error| {
            protocol_storage_error::<DB>("decode projection query registered topology", error)
        })?;
    let Some(registered_topology) = registered_topology else {
        return Err(ProjectionProtocolError::InvalidBatch(format!(
            "projection query model `{}` has no registered topology owner",
            request.scope.model()
        )));
    };
    verify_bytes(
        &registered_topology,
        &request.scope.topology().canonical_bytes(),
        "projection query registered topology",
    )?;
    let registered_table: Option<String> =
        first.try_get("ps_registered_table").map_err(|error| {
            protocol_storage_error::<DB>("decode projection query registered table", error)
        })?;
    if registered_table.as_deref() != Some(schema.table_name.as_str()) {
        return Err(ProjectionProtocolError::InvalidBatch(format!(
            "projection query model `{}` is registered to table `{}`, not `{}`",
            request.scope.model(),
            registered_table.as_deref().unwrap_or("<missing>"),
            schema.table_name
        )));
    }

    let active_generation: Option<i64> =
        first.try_get("ps_active_generation").map_err(|error| {
            protocol_storage_error::<DB>("decode projection query partition presence", error)
        })?;
    let partition = match active_generation {
        Some(active_generation) => {
            let topology_bytes: Option<Vec<u8>> = first
                .try_get("ps_partition_topology_bytes")
                .map_err(|error| {
                    protocol_storage_error::<DB>(
                        "decode projection query partition topology",
                        error,
                    )
                })?;
            let partition_bytes: Option<Vec<u8>> =
                first.try_get("ps_partition_bytes").map_err(|error| {
                    protocol_storage_error::<DB>("decode projection query partition bytes", error)
                })?;
            verify_bytes(
                topology_bytes
                    .as_deref()
                    .ok_or_else(|| corrupt_storage("partition topology bytes are missing"))?,
                &request.scope.topology().canonical_bytes(),
                "projection query partition topology",
            )?;
            verify_bytes(
                partition_bytes
                    .as_deref()
                    .ok_or_else(|| corrupt_storage("partition bytes are missing"))?,
                request.scope.projection_partition().canonical_bytes(),
                "projection query partition",
            )?;
            let change_epoch: Option<String> =
                first.try_get("ps_change_epoch").map_err(|error| {
                    protocol_storage_error::<DB>("decode projection query change epoch", error)
                })?;
            let change_head: Option<i64> = first.try_get("ps_change_head").map_err(|error| {
                protocol_storage_error::<DB>("decode projection query change head", error)
            })?;
            let compacted_through: Option<i64> =
                first.try_get("ps_compacted").map_err(|error| {
                    protocol_storage_error::<DB>(
                        "decode projection query compaction watermark",
                        error,
                    )
                })?;
            let state = PartitionState {
                active_generation: ProjectionGeneration::new(from_i64::<DB>(
                    active_generation,
                    "projection active generation",
                )?)?,
                change_epoch: ProjectionEpoch::new(
                    change_epoch
                        .ok_or_else(|| corrupt_storage("partition change epoch is missing"))?,
                )?,
                change_head: from_i64::<DB>(
                    change_head
                        .ok_or_else(|| corrupt_storage("partition change head is missing"))?,
                    "projection change head",
                )?,
                compacted_through: from_i64::<DB>(
                    compacted_through.ok_or_else(|| {
                        corrupt_storage("partition compaction watermark is missing")
                    })?,
                    "projection compaction watermark",
                )?,
                pending_retry_failure_id: None,
                stopped_failure_id: None,
            };
            if state.compacted_through > state.change_head {
                return Err(corrupt_storage(
                    "projection compaction watermark exceeds change head",
                ));
            }
            Some(state)
        }
        None => None,
    };

    let record_incarnation: Option<i64> =
        first.try_get("ps_record_incarnation").map_err(|error| {
            protocol_storage_error::<DB>("decode projection query record presence", error)
        })?;
    let record = match record_incarnation {
        Some(incarnation) => {
            let Some(partition) = partition.as_ref() else {
                return Err(corrupt_storage(
                    "projection record exists without partition state",
                ));
            };
            let key_bytes: Option<Vec<u8>> =
                first.try_get("ps_record_key_bytes").map_err(|error| {
                    protocol_storage_error::<DB>("decode projection query record key bytes", error)
                })?;
            let stored_key_hash: Option<Vec<u8>> =
                first.try_get("ps_record_key_hash").map_err(|error| {
                    protocol_storage_error::<DB>("decode projection query record key hash", error)
                })?;
            verify_bytes(
                key_bytes
                    .as_deref()
                    .ok_or_else(|| corrupt_storage("projection record key bytes are missing"))?,
                request.scope.canonical_key_bytes(),
                "projection query record key",
            )?;
            verify_digest(
                stored_key_hash
                    .as_deref()
                    .ok_or_else(|| corrupt_storage("projection record key hash is missing"))?,
                request.scope.key_digest(),
                "projection query record key",
            )?;
            let revision: Option<i64> = first.try_get("ps_record_revision").map_err(|error| {
                protocol_storage_error::<DB>("decode projection query record revision", error)
            })?;
            let tombstone: Option<i64> = first.try_get("ps_record_tombstone").map_err(|error| {
                protocol_storage_error::<DB>("decode projection query record tombstone", error)
            })?;
            let tombstone = match tombstone
                .ok_or_else(|| corrupt_storage("projection record tombstone is missing"))?
            {
                0 => false,
                1 => true,
                value => {
                    return Err(corrupt_storage(format!(
                        "record tombstone contains invalid value {value}"
                    )))
                }
            };
            let record_change_epoch: Option<String> =
                first.try_get("ps_record_change_epoch").map_err(|error| {
                    protocol_storage_error::<DB>(
                        "decode projection query record change epoch",
                        error,
                    )
                })?;
            let record_change_epoch = ProjectionEpoch::new(
                record_change_epoch
                    .ok_or_else(|| corrupt_storage("record change epoch is missing"))?,
            )?;
            if record_change_epoch != partition.change_epoch {
                return Err(corrupt_storage(
                    "projection record change epoch differs from its partition",
                ));
            }
            let record_change_position: Option<i64> = first
                .try_get("ps_record_change_position")
                .map_err(|error| {
                    protocol_storage_error::<DB>(
                        "decode projection query record change position",
                        error,
                    )
                })?;
            let record_change_position = from_i64::<DB>(
                record_change_position
                    .ok_or_else(|| corrupt_storage("record change position is missing"))?,
                "projection record change position",
            )?;
            if record_change_position > partition.change_head {
                return Err(corrupt_storage(
                    "projection record change exceeds its partition head",
                ));
            }
            Some(ProjectionRecordMetadata {
                revision: RecordRevision::new(
                    request.scope.clone(),
                    from_i64::<DB>(incarnation, "projection record incarnation")?,
                    from_i64::<DB>(
                        revision.ok_or_else(|| corrupt_storage("record revision is missing"))?,
                        "projection record revision",
                    )?,
                )?,
                tombstone,
                change: ProjectionChangeCursor::new(
                    request.scope.topology().clone(),
                    request.scope.projection_partition().clone(),
                    record_change_epoch,
                    record_change_position,
                )?,
            })
        }
        None => None,
    };

    match (physical_row.is_some(), record.as_ref()) {
        (true, None)
        | (
            true,
            Some(ProjectionRecordMetadata {
                tombstone: true, ..
            }),
        ) => {
            return Err(ProjectionProtocolError::RecordAlreadyExists {
                model: request.scope.model().to_string(),
            });
        }
        (
            false,
            Some(ProjectionRecordMetadata {
                tombstone: false, ..
            }),
        ) => {
            return Err(ProjectionProtocolError::RecordMissing {
                model: request.scope.model().to_string(),
            });
        }
        _ => {}
    }

    let mut checkpoint_values = vec![None; request.checkpoint_probes.len()];
    for row in &rows {
        let stored_generation: Option<i64> =
            row.try_get("ps_cursor_generation").map_err(|error| {
                protocol_storage_error::<DB>("decode projection query checkpoint presence", error)
            })?;
        let Some(stored_generation) = stored_generation else {
            continue;
        };
        let Some(partition) = partition.as_ref() else {
            return Err(corrupt_storage(
                "projection checkpoint exists without partition state",
            ));
        };
        let generation = ProjectionGeneration::new(from_i64::<DB>(
            stored_generation,
            "projection checkpoint generation",
        )?)?;
        let source_bytes: Option<Vec<u8>> =
            row.try_get("ps_cursor_source_bytes").map_err(|error| {
                protocol_storage_error::<DB>(
                    "decode projection query checkpoint source bytes",
                    error,
                )
            })?;
        let source_hash: Option<Vec<u8>> =
            row.try_get("ps_cursor_source_hash").map_err(|error| {
                protocol_storage_error::<DB>(
                    "decode projection query checkpoint source hash",
                    error,
                )
            })?;
        let source_partition_bytes: Option<Vec<u8>> =
            row.try_get("ps_cursor_partition_bytes").map_err(|error| {
                protocol_storage_error::<DB>(
                    "decode projection query checkpoint source partition bytes",
                    error,
                )
            })?;
        let source_partition_hash: Option<Vec<u8>> =
            row.try_get("ps_cursor_partition_hash").map_err(|error| {
                protocol_storage_error::<DB>(
                    "decode projection query checkpoint source partition hash",
                    error,
                )
            })?;
        let source_hash = source_hash
            .as_deref()
            .ok_or_else(|| corrupt_storage("checkpoint source hash is missing"))?;
        let source_partition_hash = source_partition_hash
            .as_deref()
            .ok_or_else(|| corrupt_storage("checkpoint source partition hash is missing"))?;
        let Some(probe_index) = request.checkpoint_probes.iter().position(|probe| {
            probe.generation == generation
                && source_hash == probe.source.digest().as_slice()
                && source_partition_hash == probe.source.partition_digest().as_slice()
        }) else {
            return Err(corrupt_storage(
                "checkpoint row does not match an explicit query probe",
            ));
        };
        let probe = &request.checkpoint_probes[probe_index];
        verify_bytes(
            source_bytes
                .as_deref()
                .ok_or_else(|| corrupt_storage("checkpoint source bytes are missing"))?,
            &probe.source.canonical_name_bytes(),
            "projection query checkpoint source",
        )?;
        verify_digest(
            source_hash,
            probe.source.digest(),
            "projection query checkpoint source",
        )?;
        verify_bytes(
            source_partition_bytes
                .as_deref()
                .ok_or_else(|| corrupt_storage("checkpoint source partition bytes are missing"))?,
            probe.source.canonical_partition_bytes(),
            "projection query checkpoint source partition",
        )?;
        verify_digest(
            source_partition_hash,
            probe.source.partition_digest(),
            "projection query checkpoint source partition",
        )?;
        let source_epoch: Option<String> =
            row.try_get("ps_cursor_source_epoch").map_err(|error| {
                protocol_storage_error::<DB>(
                    "decode projection query checkpoint source epoch",
                    error,
                )
            })?;
        let source_epoch = ProjectionEpoch::new(
            source_epoch.ok_or_else(|| corrupt_storage("checkpoint source epoch is missing"))?,
        )?;
        if source_epoch != probe.epoch {
            return Err(ProjectionProtocolError::IncomparableInput);
        }
        let source_position: Option<i64> =
            row.try_get("ps_cursor_source_position").map_err(|error| {
                protocol_storage_error::<DB>(
                    "decode projection query checkpoint source position",
                    error,
                )
            })?;
        let gap_free: Option<i64> = row.try_get("ps_cursor_gap_free").map_err(|error| {
            protocol_storage_error::<DB>("decode projection query checkpoint gap-free flag", error)
        })?;
        let gap_free =
            match gap_free.ok_or_else(|| corrupt_storage("checkpoint gap-free flag is missing"))? {
                0 => false,
                1 => true,
                value => {
                    return Err(corrupt_storage(format!(
                        "cursor gap-free flag contains invalid value {value}"
                    )))
                }
            };
        let checkpoint_change_epoch: Option<String> =
            row.try_get("ps_cursor_change_epoch").map_err(|error| {
                protocol_storage_error::<DB>(
                    "decode projection query checkpoint change epoch",
                    error,
                )
            })?;
        let checkpoint_change_epoch = ProjectionEpoch::new(
            checkpoint_change_epoch
                .ok_or_else(|| corrupt_storage("checkpoint change epoch is missing"))?,
        )?;
        let checkpoint_change_position: Option<i64> =
            row.try_get("ps_cursor_change_position").map_err(|error| {
                protocol_storage_error::<DB>(
                    "decode projection query checkpoint change position",
                    error,
                )
            })?;
        let checkpoint_change_position = from_i64::<DB>(
            checkpoint_change_position
                .ok_or_else(|| corrupt_storage("checkpoint change position is missing"))?,
            "projection checkpoint change position",
        )?;
        if checkpoint_change_epoch != partition.change_epoch
            || checkpoint_change_position > partition.change_head
        {
            return Err(corrupt_storage(
                "projection checkpoint change lies outside its partition head",
            ));
        }
        let checkpoint = ProjectionCheckpoint::new(
            ProjectionInputCursor::new(
                probe.topology.clone(),
                probe.partition.clone(),
                probe.source.clone(),
                source_epoch,
                from_i64::<DB>(
                    source_position
                        .ok_or_else(|| corrupt_storage("checkpoint source position is missing"))?,
                    "projection checkpoint source position",
                )?,
            )?,
            ProjectionChangeCursor::new(
                probe.topology.clone(),
                probe.partition.clone(),
                checkpoint_change_epoch,
                checkpoint_change_position,
            )?,
            gap_free,
        )?;
        if checkpoint_values[probe_index].replace(checkpoint).is_some() {
            return Err(corrupt_storage(
                "projection query returned duplicate checkpoint rows",
            ));
        }
    }

    let checkpoints = request
        .checkpoint_probes
        .iter()
        .cloned()
        .zip(checkpoint_values)
        .map(
            |(probe, checkpoint)| crate::projection_protocol::ProjectionCheckpointSnapshot {
                probe,
                checkpoint,
            },
        )
        .collect();
    let (change_head, compacted_through) = match partition {
        Some(partition) => {
            let head = if partition.change_head == 0 {
                None
            } else {
                Some(ProjectionChangeCursor::new(
                    request.scope.topology().clone(),
                    request.scope.projection_partition().clone(),
                    partition.change_epoch,
                    partition.change_head,
                )?)
            };
            (head, partition.compacted_through)
        }
        None => (None, 0),
    };

    Ok(ProjectionQuerySnapshot {
        row: physical_row,
        record,
        checkpoints,
        change_head,
        compacted_through,
    })
}

/// Resolve a bounded command-obligation set from one existing SQL snapshot.
///
/// Observations and failures are read in two set queries on the same borrowed
/// connection. A durable failure is immutable and therefore wins even when an
/// observation for the same causation also exists or its change entry has been
/// compacted.
pub(crate) async fn read_projection_obligation_evidence_batch_in_executor<DB>(
    connection: &mut DB::Connection,
    request: &ProjectionObligationEvidenceBatchRequest,
) -> Result<ProjectionObligationEvidenceBatch, ProjectionProtocolError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> String: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> Vec<u8>: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
    for<'q> &'q [u8]: Encode<'q, DB> + Type<DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    request.validate()?;
    if request.requests.is_empty() {
        return Ok(ProjectionObligationEvidenceBatch::default());
    }

    let mut observations = vec![None; request.requests.len()];
    let mut observation_query = QueryBuilder::<DB>::new(
        "SELECT observation.topology_hash AS evidence_topology_hash, \
         observation.partition_hash AS evidence_partition_hash, \
         observation.causation_id AS evidence_causation_id, \
         observation.model_name AS evidence_model_name, \
         observation.scope_kind AS evidence_scope_kind, \
         observation.canonical_key_bytes, observation.canonical_key_hash, \
         observation.incarnation, observation.revision, observation.change_epoch, \
         observation.change_position, partition.topology_bytes AS evidence_topology_bytes, \
         partition.partition_bytes AS evidence_partition_bytes, \
         partition.change_epoch AS evidence_partition_epoch, \
         partition.change_head AS evidence_partition_head \
         FROM projection_observations AS observation \
         INNER JOIN projection_partitions AS partition \
         ON partition.topology_hash = observation.topology_hash \
         AND partition.partition_hash = observation.partition_hash WHERE ",
    );
    for (index, probe) in request.requests.iter().enumerate() {
        if index > 0 {
            observation_query.push(" OR ");
        }
        let topology_hash = probe.scope.topology().digest();
        let partition_hash = probe.scope.projection_partition().digest();
        let key_hash = probe.scope.key_digest();
        observation_query.push("(observation.topology_hash = ");
        observation_query.push_bind(topology_hash.as_slice());
        observation_query.push(" AND observation.partition_hash = ");
        observation_query.push_bind(partition_hash.as_slice());
        observation_query.push(" AND observation.causation_id = ");
        observation_query.push_bind(probe.causation_id.as_str());
        observation_query.push(" AND observation.model_name = ");
        observation_query.push_bind(probe.scope.model());
        observation_query.push(" AND observation.scope_kind = ");
        observation_query.push_bind(probe.kind.as_storage_str());
        observation_query.push(" AND observation.canonical_key_hash = ");
        observation_query.push_bind(key_hash.as_slice());
        observation_query.push(")");
    }
    let observation_rows = observation_query
        .build()
        .fetch_all(&mut *connection)
        .await
        .map_err(|error| {
            protocol_storage_error::<DB>("read projection obligation observations", error)
        })?;
    for row in observation_rows {
        let topology_hash = decode_digest(
            row.try_get("evidence_topology_hash").map_err(|error| {
                protocol_storage_error::<DB>("decode evidence topology hash", error)
            })?,
            "projection evidence topology",
        )?;
        let partition_hash = decode_digest(
            row.try_get("evidence_partition_hash").map_err(|error| {
                protocol_storage_error::<DB>("decode evidence partition hash", error)
            })?,
            "projection evidence partition",
        )?;
        let causation_id: String = row
            .try_get("evidence_causation_id")
            .map_err(|error| protocol_storage_error::<DB>("decode evidence causation ID", error))?;
        let model: String = row
            .try_get("evidence_model_name")
            .map_err(|error| protocol_storage_error::<DB>("decode evidence model", error))?;
        let kind_value: String = row.try_get("evidence_scope_kind").map_err(|error| {
            protocol_storage_error::<DB>("decode evidence observation kind", error)
        })?;
        let kind = decode_observation_kind(&kind_value)?;
        let key_hash = decode_digest(
            row.try_get("canonical_key_hash")
                .map_err(|error| protocol_storage_error::<DB>("decode evidence key hash", error))?,
            "projection evidence key",
        )?;
        let key_bytes: Vec<u8> = row
            .try_get("canonical_key_bytes")
            .map_err(|error| protocol_storage_error::<DB>("decode evidence key bytes", error))?;
        let digest_candidates = request
            .requests
            .iter()
            .enumerate()
            .filter(|(_, probe)| {
                probe.scope.topology().digest() == topology_hash
                    && probe.scope.projection_partition().digest() == partition_hash
                    && probe.causation_id == causation_id
                    && probe.scope.model() == model
                    && probe.kind == kind
                    && probe.scope.key_digest() == key_hash
            })
            .collect::<Vec<_>>();
        let exact = digest_candidates
            .iter()
            .filter(|(_, probe)| probe.scope.canonical_key_bytes() == key_bytes.as_slice())
            .map(|(index, _)| *index)
            .collect::<Vec<_>>();
        let [index] = exact.as_slice() else {
            return Err(corrupt_storage(if digest_candidates.is_empty() {
                "projection observation escaped its bounded evidence predicate".to_string()
            } else {
                "projection observation canonical key does not match its digest lookup".to_string()
            }));
        };
        let probe = &request.requests[*index];
        let topology_bytes: Vec<u8> = row.try_get("evidence_topology_bytes").map_err(|error| {
            protocol_storage_error::<DB>("decode evidence topology bytes", error)
        })?;
        verify_bytes(
            &topology_bytes,
            &probe.scope.topology().canonical_bytes(),
            "projection evidence topology",
        )?;
        let partition_bytes: Vec<u8> =
            row.try_get("evidence_partition_bytes").map_err(|error| {
                protocol_storage_error::<DB>("decode evidence partition bytes", error)
            })?;
        verify_bytes(
            &partition_bytes,
            probe.scope.projection_partition().canonical_bytes(),
            "projection evidence partition",
        )?;
        let partition_epoch = ProjectionEpoch::new(
            row.try_get::<String, _>("evidence_partition_epoch")
                .map_err(|error| {
                    protocol_storage_error::<DB>("decode evidence partition epoch", error)
                })?,
        )?;
        let partition_head = from_i64::<DB>(
            row.try_get("evidence_partition_head").map_err(|error| {
                protocol_storage_error::<DB>("decode evidence partition head", error)
            })?,
            "projection evidence partition head",
        )?;
        let observation = decode_observation_row::<DB>(
            &row,
            &probe.causation_id,
            &probe.scope,
            probe.kind,
            &partition_epoch,
        )?;
        if observation.change.position() > partition_head {
            return Err(corrupt_storage(
                "projection observation change exceeds its partition head",
            ));
        }
        if observations[*index].replace(observation).is_some() {
            return Err(corrupt_storage(
                "projection obligation evidence returned duplicate observations",
            ));
        }
    }

    let mut failures = vec![None; request.requests.len()];
    let mut failure_query = QueryBuilder::<DB>::new(
        "SELECT failure.topology_hash AS evidence_topology_hash, \
         failure.partition_hash AS evidence_partition_hash, failure.failure_id, \
         failure.source_bytes, failure.source_hash, failure.source_partition_bytes, \
         failure.source_partition_hash, failure.source_epoch, failure.source_position, \
         failure.input_hash, failure.message_id, failure.causation_id, failure.gap_free, \
         failure.generation, failure.failure_code, failure.failure_bytes, \
         failure.failure_hash, failure.change_epoch, failure.change_position, \
         partition.topology_bytes AS evidence_topology_bytes, \
         partition.partition_bytes AS evidence_partition_bytes, \
         partition.change_epoch AS evidence_partition_epoch, \
         partition.change_head AS evidence_partition_head \
         FROM projection_failures AS failure \
         INNER JOIN projection_partitions AS partition \
         ON partition.topology_hash = failure.topology_hash \
         AND partition.partition_hash = failure.partition_hash WHERE ",
    );
    for (index, probe) in request.requests.iter().enumerate() {
        if index > 0 {
            failure_query.push(" OR ");
        }
        let topology_hash = probe.scope.topology().digest();
        let partition_hash = probe.scope.projection_partition().digest();
        failure_query.push("(failure.topology_hash = ");
        failure_query.push_bind(topology_hash.as_slice());
        failure_query.push(" AND failure.partition_hash = ");
        failure_query.push_bind(partition_hash.as_slice());
        failure_query.push(" AND failure.causation_id = ");
        failure_query.push_bind(probe.causation_id.as_str());
        failure_query.push(")");
    }
    failure_query.push(
        " ORDER BY failure.topology_hash, failure.partition_hash, \
         failure.causation_id, failure.change_position, failure.failure_id",
    );
    let failure_rows = failure_query
        .build()
        .fetch_all(&mut *connection)
        .await
        .map_err(|error| {
            protocol_storage_error::<DB>("read projection obligation failures", error)
        })?;
    for row in failure_rows {
        let topology_hash = decode_digest(
            row.try_get("evidence_topology_hash").map_err(|error| {
                protocol_storage_error::<DB>("decode failure evidence topology hash", error)
            })?,
            "projection failure evidence topology",
        )?;
        let partition_hash = decode_digest(
            row.try_get("evidence_partition_hash").map_err(|error| {
                protocol_storage_error::<DB>("decode failure evidence partition hash", error)
            })?,
            "projection failure evidence partition",
        )?;
        let causation_id: String = row.try_get("causation_id").map_err(|error| {
            protocol_storage_error::<DB>("decode failure evidence causation ID", error)
        })?;
        let matching = request
            .requests
            .iter()
            .enumerate()
            .filter(|(_, probe)| {
                probe.scope.topology().digest() == topology_hash
                    && probe.scope.projection_partition().digest() == partition_hash
                    && probe.causation_id == causation_id
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        let Some(first_index) = matching.first().copied() else {
            return Err(corrupt_storage(
                "projection failure escaped its bounded evidence predicate",
            ));
        };
        let first = &request.requests[first_index];
        let topology_bytes: Vec<u8> = row.try_get("evidence_topology_bytes").map_err(|error| {
            protocol_storage_error::<DB>("decode failure evidence topology bytes", error)
        })?;
        verify_bytes(
            &topology_bytes,
            &first.scope.topology().canonical_bytes(),
            "projection failure evidence topology",
        )?;
        let partition_bytes: Vec<u8> =
            row.try_get("evidence_partition_bytes").map_err(|error| {
                protocol_storage_error::<DB>("decode failure evidence partition bytes", error)
            })?;
        verify_bytes(
            &partition_bytes,
            first.scope.projection_partition().canonical_bytes(),
            "projection failure evidence partition",
        )?;
        let partition_epoch = ProjectionEpoch::new(
            row.try_get::<String, _>("evidence_partition_epoch")
                .map_err(|error| {
                    protocol_storage_error::<DB>("decode failure evidence partition epoch", error)
                })?,
        )?;
        let partition_head = from_i64::<DB>(
            row.try_get("evidence_partition_head").map_err(|error| {
                protocol_storage_error::<DB>("decode failure evidence partition head", error)
            })?,
            "projection failure evidence partition head",
        )?;
        let failure = decode_failure_row::<DB>(
            &row,
            first.scope.topology(),
            first.scope.projection_partition(),
            &partition_epoch,
        )?
        .failure;
        if failure.causation_id != causation_id || failure.change.position() > partition_head {
            return Err(corrupt_storage(
                "projection failure evidence lies outside its exact partition/causation",
            ));
        }
        // Rows are ordered by change position, so retaining the first durable
        // failure preserves the command promise's original terminal outcome.
        for index in matching {
            if failures[index].is_none() {
                failures[index] = Some(failure.clone());
            }
        }
    }

    let evidence = failures
        .into_iter()
        .zip(observations)
        .map(|(failure, observation)| match (failure, observation) {
            (Some(failure), _) => ProjectionObligationEvidence::TerminalFailure(failure),
            (None, Some(observation)) => ProjectionObligationEvidence::Observed(observation),
            (None, None) => ProjectionObligationEvidence::Pending,
        })
        .collect();
    Ok(ProjectionObligationEvidenceBatch { evidence })
}

/// Recover exact live record scopes for typed physical-row keys without
/// requiring the caller to know hidden projection partitions.
pub(crate) async fn read_projection_live_record_batch_in_executor<DB>(
    connection: &mut DB::Connection,
    request: &ProjectionLiveRecordBatchRequest,
) -> Result<ProjectionLiveRecordBatch, ProjectionProtocolError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> String: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> Vec<u8>: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
    for<'q> &'q [u8]: Encode<'q, DB> + Type<DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    request.validate()?;
    if request.requests.is_empty() {
        return Ok(ProjectionLiveRecordBatch::default());
    }

    let mut registered = vec![false; request.requests.len()];
    let mut registration_query = QueryBuilder::<DB>::new(
        "SELECT topology_bytes, topology_hash, model_name, table_name \
         FROM projection_registered_models WHERE ",
    );
    for (index, probe) in request.requests.iter().enumerate() {
        if index > 0 {
            registration_query.push(" OR ");
        }
        let topology_hash = probe.topology.digest();
        registration_query.push("(topology_hash = ");
        registration_query.push_bind(topology_hash.as_slice());
        registration_query.push(" AND model_name = ");
        registration_query.push_bind(probe.model());
        registration_query.push(")");
    }
    let registration_rows = registration_query
        .build()
        .fetch_all(&mut *connection)
        .await
        .map_err(|error| {
            protocol_storage_error::<DB>("read projection live-record registrations", error)
        })?;
    for row in registration_rows {
        let topology_hash = decode_digest(
            row.try_get("topology_hash").map_err(|error| {
                protocol_storage_error::<DB>("decode live-record topology hash", error)
            })?,
            "projection live-record topology",
        )?;
        let model: String = row.try_get("model_name").map_err(|error| {
            protocol_storage_error::<DB>("decode live-record registered model", error)
        })?;
        let topology_bytes: Vec<u8> = row.try_get("topology_bytes").map_err(|error| {
            protocol_storage_error::<DB>("decode live-record topology bytes", error)
        })?;
        let table: String = row.try_get("table_name").map_err(|error| {
            protocol_storage_error::<DB>("decode live-record registered table", error)
        })?;
        let matching = request
            .requests
            .iter()
            .enumerate()
            .filter(|(_, probe)| probe.topology.digest() == topology_hash && probe.model() == model)
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        if matching.is_empty() {
            return Err(corrupt_storage(
                "projection registration escaped its bounded live-record predicate",
            ));
        }
        for index in matching {
            let probe = &request.requests[index];
            verify_bytes(
                &topology_bytes,
                &probe.topology.canonical_bytes(),
                "projection live-record topology",
            )?;
            if table != probe.schema.table_name {
                return Err(ProjectionProtocolError::InvalidBatch(format!(
                    "projection live-record model `{}` is registered to table `{table}`, not `{}`",
                    probe.model(),
                    probe.schema.table_name
                )));
            }
            if std::mem::replace(&mut registered[index], true) {
                return Err(corrupt_storage(
                    "projection live-record model has duplicate registrations",
                ));
            }
        }
    }
    for (index, is_registered) in registered.into_iter().enumerate() {
        if !is_registered {
            return Err(ProjectionProtocolError::InvalidBatch(format!(
                "projection live-record model `{}` has no registered topology owner",
                request.requests[index].model()
            )));
        }
    }

    let mut record_query = QueryBuilder::<DB>::new(
        "SELECT record.topology_hash AS live_topology_hash, record.partition_hash AS \
         live_partition_hash, record.model_name AS live_model_name, \
         record.canonical_key_bytes, record.canonical_key_hash, record.incarnation, \
         record.revision, record.tombstone, record.change_epoch, record.change_position, \
         partition.topology_bytes AS live_topology_bytes, \
         partition.partition_bytes AS live_partition_bytes, \
         partition.change_epoch AS live_partition_epoch, \
         partition.change_head AS live_partition_head \
         FROM projection_records AS record \
         INNER JOIN projection_partitions AS partition \
         ON partition.topology_hash = record.topology_hash \
         AND partition.partition_hash = record.partition_hash \
         WHERE record.tombstone = 0 AND (",
    );
    for (index, probe) in request.requests.iter().enumerate() {
        if index > 0 {
            record_query.push(" OR ");
        }
        let topology_hash = probe.topology.digest();
        record_query.push("(record.topology_hash = ");
        record_query.push_bind(topology_hash.as_slice());
        record_query.push(" AND record.model_name = ");
        record_query.push_bind(probe.model());
        record_query.push(" AND record.canonical_key_hash = ");
        record_query.push_bind(probe.canonical_key_hash.as_slice());
        record_query.push(")");
    }
    record_query.push(")");
    let rows = record_query
        .build()
        .fetch_all(&mut *connection)
        .await
        .map_err(|error| {
            protocol_storage_error::<DB>("read projection live-record evidence", error)
        })?;
    let mut records = vec![None; request.requests.len()];
    for row in rows {
        let topology_hash = decode_digest(
            row.try_get("live_topology_hash").map_err(|error| {
                protocol_storage_error::<DB>("decode live-record topology hash", error)
            })?,
            "projection live-record topology",
        )?;
        let model: String = row
            .try_get("live_model_name")
            .map_err(|error| protocol_storage_error::<DB>("decode live-record model", error))?;
        let key_hash = decode_digest(
            row.try_get("canonical_key_hash").map_err(|error| {
                protocol_storage_error::<DB>("decode live-record key hash", error)
            })?,
            "projection live-record key",
        )?;
        let key_bytes: Vec<u8> = row
            .try_get("canonical_key_bytes")
            .map_err(|error| protocol_storage_error::<DB>("decode live-record key bytes", error))?;
        let digest_candidates = request
            .requests
            .iter()
            .enumerate()
            .filter(|(_, probe)| {
                probe.topology.digest() == topology_hash
                    && probe.model() == model
                    && probe.canonical_key_hash == key_hash
            })
            .collect::<Vec<_>>();
        let exact = digest_candidates
            .iter()
            .filter(|(_, probe)| probe.canonical_key_bytes == key_bytes)
            .map(|(index, _)| *index)
            .collect::<Vec<_>>();
        let [index] = exact.as_slice() else {
            return Err(corrupt_storage(if digest_candidates.is_empty() {
                "projection record escaped its bounded live-record predicate".to_string()
            } else {
                "projection live-record canonical key does not match its digest lookup".to_string()
            }));
        };
        let probe = &request.requests[*index];
        let topology_bytes: Vec<u8> = row.try_get("live_topology_bytes").map_err(|error| {
            protocol_storage_error::<DB>("decode live-record topology bytes", error)
        })?;
        verify_bytes(
            &topology_bytes,
            &probe.topology.canonical_bytes(),
            "projection live-record topology",
        )?;
        let partition_bytes: Vec<u8> = row.try_get("live_partition_bytes").map_err(|error| {
            protocol_storage_error::<DB>("decode live-record partition bytes", error)
        })?;
        let partition = ProjectionPartition::new(partition_bytes)?;
        let stored_partition_hash: Vec<u8> =
            row.try_get("live_partition_hash").map_err(|error| {
                protocol_storage_error::<DB>("decode live-record partition hash", error)
            })?;
        verify_digest(
            &stored_partition_hash,
            partition.digest(),
            "projection live-record partition",
        )?;
        let scope = ProjectionRecordScope::new(
            probe.topology.clone(),
            partition,
            probe.model().to_string(),
            key_bytes,
        )?;
        verify_digest(&key_hash, scope.key_digest(), "projection live-record key")?;
        let incarnation = from_i64::<DB>(
            row.try_get("incarnation").map_err(|error| {
                protocol_storage_error::<DB>("decode live-record incarnation", error)
            })?,
            "projection live-record incarnation",
        )?;
        let revision = from_i64::<DB>(
            row.try_get("revision").map_err(|error| {
                protocol_storage_error::<DB>("decode live-record revision", error)
            })?,
            "projection live-record revision",
        )?;
        let tombstone: i64 = row
            .try_get("tombstone")
            .map_err(|error| protocol_storage_error::<DB>("decode live-record tombstone", error))?;
        if tombstone != 0 {
            return Err(corrupt_storage(
                "projection live-record lookup returned a tombstone",
            ));
        }
        let partition_epoch =
            ProjectionEpoch::new(row.try_get::<String, _>("live_partition_epoch").map_err(
                |error| protocol_storage_error::<DB>("decode live-record partition epoch", error),
            )?)?;
        let change_epoch =
            ProjectionEpoch::new(row.try_get::<String, _>("change_epoch").map_err(|error| {
                protocol_storage_error::<DB>("decode live-record change epoch", error)
            })?)?;
        if change_epoch != partition_epoch {
            return Err(corrupt_storage(
                "projection live-record change epoch differs from its partition",
            ));
        }
        let change_position = from_i64::<DB>(
            row.try_get("change_position").map_err(|error| {
                protocol_storage_error::<DB>("decode live-record change position", error)
            })?,
            "projection live-record change position",
        )?;
        let partition_head = from_i64::<DB>(
            row.try_get("live_partition_head").map_err(|error| {
                protocol_storage_error::<DB>("decode live-record partition head", error)
            })?,
            "projection live-record partition head",
        )?;
        if change_position > partition_head {
            return Err(corrupt_storage(
                "projection live-record change exceeds its partition head",
            ));
        }
        let metadata = ProjectionRecordMetadata {
            revision: RecordRevision::new(scope.clone(), incarnation, revision)?,
            tombstone: false,
            change: ProjectionChangeCursor::new(
                scope.topology().clone(),
                scope.projection_partition().clone(),
                change_epoch,
                change_position,
            )?,
        };
        if records[*index].replace(metadata).is_some() {
            return Err(corrupt_storage(format!(
                "projection live-record identity for model `{}` is ambiguous across partitions",
                probe.model()
            )));
        }
    }
    Ok(ProjectionLiveRecordBatch { records })
}

/// Boxed operation run inside one framework-owned projection read snapshot.
///
/// The boxed lifetime prevents callers from leaking the borrowed connection
/// beyond the transaction boundary.
pub(crate) type ProjectionReadSnapshotFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, ProjectionProtocolError>> + Send + 'a>>;

/// Run a physical query plan and all of its causal-evidence probes in one
/// repeatable database snapshot.
///
/// PostgreSQL uses `REPEATABLE READ READ ONLY`; its default READ COMMITTED
/// isolation would let each metadata statement observe a newer commit than the
/// physical GraphQL statement. SQLite holds one ordinary read transaction,
/// whose first read establishes the snapshot for the remaining plan.
pub(crate) async fn with_projection_read_snapshot<DB, T, F>(
    pool: &Pool<DB>,
    operation: F,
) -> Result<T, ProjectionProtocolError>
where
    DB: SqlxRepoBackend,
    T: Send,
    F: for<'connection> FnOnce(
            &'connection mut DB::Connection,
        ) -> ProjectionReadSnapshotFuture<'connection, T>
        + Send,
    DB::Arguments: IntoArguments<DB>,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
{
    let mut tx = pool
        .begin()
        .await
        .map_err(|error| protocol_storage_error::<DB>("begin projection read snapshot", error))?;
    if DB::BACKEND == "postgres" {
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await
            .map_err(|error| {
                protocol_storage_error::<DB>("configure projection read snapshot", error)
            })?;
    }

    let result = operation(&mut *tx).await;
    match result {
        Ok(value) => {
            tx.commit().await.map_err(|error| {
                protocol_storage_error::<DB>("commit projection read snapshot", error)
            })?;
            Ok(value)
        }
        Err(error) => {
            tx.rollback().await.map_err(|rollback_error| {
                protocol_storage_error::<DB>(
                    "roll back failed projection read snapshot",
                    rollback_error,
                )
            })?;
            Err(error)
        }
    }
}

/// Read one resumable projection-change page from a single database snapshot.
///
/// `after_state` is normally an immediately-ready future. Tests use it to
/// commit compaction after the partition watermark has been observed but
/// before retained rows are read, proving both statements remain one view.
async fn read_projection_changes_in_snapshot<DB, AfterState>(
    pool: &Pool<DB>,
    topology: ProjectorTopologyId,
    partition: ProjectionPartition,
    after: Option<ProjectionChangeCursor>,
    limit: usize,
    after_state: AfterState,
) -> Result<ProjectionChangeRead, ProjectionProtocolError>
where
    DB: SqlxRepoBackend,
    AfterState: Future<Output = ()> + Send + 'static,
    DB::Arguments: IntoArguments<DB>,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> String: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> Vec<u8>: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
    for<'q> &'q [u8]: Encode<'q, DB> + Type<DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    with_projection_read_snapshot(pool, move |connection| {
        Box::pin(async move {
            let state = load_partition_in_connection(connection, &topology, &partition).await?;
            after_state.await;
            let Some(state) = state else {
                return Ok(match after {
                    Some(_) => ProjectionChangeRead::ResetRequired {
                        head: None,
                        compacted_through: 0,
                    },
                    None => ProjectionChangeRead::Changes {
                        head: None,
                        compacted_through: 0,
                        changes: Vec::new(),
                    },
                });
            };
            let head = if state.change_head == 0 {
                None
            } else {
                Some(ProjectionChangeCursor::new(
                    topology.clone(),
                    partition.clone(),
                    state.change_epoch.clone(),
                    state.change_head,
                )?)
            };
            if after.is_none() && state.compacted_through > 0 {
                return Ok(ProjectionChangeRead::ResetRequired {
                    head,
                    compacted_through: state.compacted_through,
                });
            }
            let start = match after.as_ref() {
                Some(cursor)
                    if cursor.topology() != &topology
                        || cursor.projection_partition() != &partition
                        || cursor.epoch() != &state.change_epoch
                        || cursor.position() > state.change_head
                        || cursor.position() < state.compacted_through =>
                {
                    return Ok(ProjectionChangeRead::ResetRequired {
                        head,
                        compacted_through: state.compacted_through,
                    });
                }
                Some(cursor) => cursor.position(),
                None => state.compacted_through,
            };
            if limit == 0 || start == state.change_head {
                return Ok(ProjectionChangeRead::Changes {
                    head,
                    compacted_through: state.compacted_through,
                    changes: Vec::new(),
                });
            }
            let topology_hash = topology.digest();
            let partition_hash = partition.digest();
            let mut builder = QueryBuilder::<DB>::new(
                "SELECT change_epoch, change_position, change_kind, causation_id, model_name, \
                 scope_kind, canonical_key_bytes, canonical_key_hash, incarnation, revision, \
                 failure_id FROM projection_changes WHERE topology_hash = ",
            );
            builder.push_bind(topology_hash.as_slice());
            builder.push(" AND partition_hash = ");
            builder.push_bind(partition_hash.as_slice());
            builder.push(" AND change_epoch = ");
            builder.push_bind(state.change_epoch.as_str());
            builder.push(" AND change_position > ");
            builder.push_bind(to_i64::<DB>(start, "projection change read position")?);
            builder.push(" ORDER BY change_position ASC LIMIT ");
            builder.push_bind(i64::try_from(limit).unwrap_or(i64::MAX));
            let rows = builder
                .build()
                .fetch_all(&mut *connection)
                .await
                .map_err(|error| protocol_storage_error::<DB>("read projection changes", error))?;
            let mut changes = Vec::with_capacity(rows.len());
            let mut expected = checked_next(start, "projection change read")?;
            for row in rows {
                let change =
                    decode_change_row::<DB>(&row, &topology, &partition, &state.change_epoch)?;
                if change.cursor.position() != expected {
                    return Err(corrupt_storage(format!(
                        "projection change log expected position {expected} but found {}",
                        change.cursor.position()
                    )));
                }
                expected = if change.cursor.position() == state.change_head {
                    state.change_head
                } else {
                    checked_next(change.cursor.position(), "projection change read")?
                };
                changes.push(change);
            }
            if changes.is_empty() {
                return Err(corrupt_storage(format!(
                    "projection change log is missing retained position {}",
                    checked_next(start, "projection change read")?
                )));
            }
            Ok(ProjectionChangeRead::Changes {
                head,
                compacted_through: state.compacted_through,
                changes,
            })
        })
    })
    .await
}

impl<DB> ProjectionProtocolStore for SqlxRepository<DB>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    for<'c> &'c Pool<DB>: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> String: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> Vec<u8>: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
    for<'q> &'q [u8]: Encode<'q, DB> + Type<DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    fn register_projection_models<'a>(
        &'a self,
        topology: &'a ProjectorTopologyId,
        ownership: &'a [ProjectionModelOwnership],
    ) -> impl Future<Output = Result<(), ProjectionProtocolError>> + Send + 'a {
        async move {
            if ownership.is_empty() {
                return Err(ProjectionProtocolError::InvalidBatch(
                    "projection startup registration requires at least one owned model".into(),
                ));
            }
            let mut models = HashMap::new();
            let mut tables = HashMap::new();
            for declaration in ownership {
                if let Some(previous) =
                    models.insert(declaration.model.as_str(), declaration.table.as_str())
                {
                    if previous != declaration.table {
                        return Err(ProjectionProtocolError::InvalidBatch(format!(
                            "projection model `{}` declares both table `{previous}` and `{}`",
                            declaration.model, declaration.table
                        )));
                    }
                }
                if let Some(previous) =
                    tables.insert(declaration.table.as_str(), declaration.model.as_str())
                {
                    if previous != declaration.model {
                        return Err(ProjectionProtocolError::InvalidBatch(format!(
                            "projection table `{}` is declared by both model `{previous}` and `{}`",
                            declaration.table, declaration.model
                        )));
                    }
                }
            }

            let mut tx = self.pool().begin().await.map_err(|error| {
                protocol_storage_error::<DB>("begin projection registration", error)
            })?;
            let ownership_tables = ownership
                .iter()
                .map(|declaration| declaration.table.clone())
                .collect::<BTreeSet<_>>();
            lock_projection_table_ownership_fences_in_tx(&mut tx, &ownership_tables).await?;
            let topology_hash = topology.digest();
            let topology_bytes = topology.canonical_bytes();
            for declaration in ownership {
                let mut global = QueryBuilder::<DB>::new(
                    "SELECT model_name FROM projection_causal_tables WHERE table_name = ",
                );
                global.push_bind(declaration.table.as_str());
                let existing = global
                    .build()
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(|error| {
                        protocol_storage_error::<DB>(
                            "load global causal projection registration",
                            error,
                        )
                    })?;
                if let Some(row) = existing {
                    let model: String = row.try_get("model_name").map_err(|error| {
                        protocol_storage_error::<DB>(
                            "decode global causal projection registration",
                            error,
                        )
                    })?;
                    if model != declaration.model {
                        return Err(ProjectionProtocolError::InvalidBatch(format!(
                            "projection table `{}` is globally registered for model `{model}`",
                            declaration.table
                        )));
                    }
                } else {
                    let mut legacy_rows = QueryBuilder::<DB>::new("SELECT 1 FROM ");
                    legacy_rows.push(quote_identifier(&declaration.table));
                    legacy_rows.push(" LIMIT 1");
                    if legacy_rows
                        .build()
                        .fetch_optional(&mut *tx)
                        .await
                        .map_err(|error| {
                            protocol_storage_error::<DB>(
                                "verify empty table before causal registration",
                                error,
                            )
                        })?
                        .is_some()
                    {
                        return Err(ProjectionProtocolError::InvalidBatch(format!(
                            "cannot causally register table `{}` because it already contains unverified legacy rows",
                            declaration.table
                        )));
                    }
                    let mut insert = QueryBuilder::<DB>::new(
                        "INSERT INTO projection_causal_tables (table_name, model_name) VALUES (",
                    );
                    insert.push_bind(declaration.table.as_str());
                    insert.push(", ");
                    insert.push_bind(declaration.model.as_str());
                    insert.push(") ON CONFLICT (table_name) DO NOTHING");
                    insert.build().execute(&mut *tx).await.map_err(|error| {
                        protocol_storage_error::<DB>(
                            "insert global causal projection registration",
                            error,
                        )
                    })?;
                    let mut verify = QueryBuilder::<DB>::new(
                        "SELECT model_name FROM projection_causal_tables WHERE table_name = ",
                    );
                    verify.push_bind(declaration.table.as_str());
                    let row = verify
                        .build()
                        .fetch_optional(&mut *tx)
                        .await
                        .map_err(|error| {
                            protocol_storage_error::<DB>(
                                "verify global causal projection registration",
                                error,
                            )
                        })?
                        .ok_or_else(|| {
                            corrupt_storage(
                                "global causal projection registration disappeared after insert",
                            )
                        })?;
                    let model: String = row.try_get("model_name").map_err(|error| {
                        protocol_storage_error::<DB>(
                            "decode global causal projection registration",
                            error,
                        )
                    })?;
                    if model != declaration.model {
                        return Err(ProjectionProtocolError::InvalidBatch(format!(
                            "projection table `{}` is globally registered for model `{model}`",
                            declaration.table
                        )));
                    }
                }

                let mut physical_owner = QueryBuilder::<DB>::new(
                    "SELECT topology_bytes, topology_hash, model_name \
                     FROM projection_registered_models WHERE table_name = ",
                );
                physical_owner.push_bind(declaration.table.as_str());
                if let Some(row) = physical_owner
                    .build()
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(|error| {
                        protocol_storage_error::<DB>(
                            "load authoritative projection table owner",
                            error,
                        )
                    })?
                {
                    let owner_hash: Vec<u8> = row.try_get("topology_hash").map_err(|error| {
                        protocol_storage_error::<DB>(
                            "decode authoritative projection table owner",
                            error,
                        )
                    })?;
                    let owner_model: String = row.try_get("model_name").map_err(|error| {
                        protocol_storage_error::<DB>(
                            "decode authoritative projection table owner",
                            error,
                        )
                    })?;
                    if owner_hash != topology_hash || owner_model != declaration.model {
                        return Err(ProjectionProtocolError::InvalidBatch(format!(
                            "projection table `{}` is authoritatively owned by another topology/model",
                            declaration.table
                        )));
                    }
                    let owner_bytes: Vec<u8> = row.try_get("topology_bytes").map_err(|error| {
                        protocol_storage_error::<DB>(
                            "decode authoritative projection topology bytes",
                            error,
                        )
                    })?;
                    verify_bytes(
                        &owner_bytes,
                        &topology_bytes,
                        "authoritative projector topology",
                    )?;
                    continue;
                }

                let mut registered = QueryBuilder::<DB>::new(
                    "SELECT topology_bytes, table_name FROM projection_registered_models \
                     WHERE topology_hash = ",
                );
                registered.push_bind(topology_hash.as_slice());
                registered.push(" AND model_name = ");
                registered.push_bind(declaration.model.as_str());
                if let Some(row) =
                    registered
                        .build()
                        .fetch_optional(&mut *tx)
                        .await
                        .map_err(|error| {
                            protocol_storage_error::<DB>(
                                "load topology projection registration",
                                error,
                            )
                        })?
                {
                    let bytes: Vec<u8> = row.try_get("topology_bytes").map_err(|error| {
                        protocol_storage_error::<DB>(
                            "decode topology projection registration",
                            error,
                        )
                    })?;
                    verify_bytes(&bytes, &topology_bytes, "registered projector topology")?;
                    let table: String = row.try_get("table_name").map_err(|error| {
                        protocol_storage_error::<DB>(
                            "decode topology projection registration",
                            error,
                        )
                    })?;
                    if table != declaration.table {
                        return Err(ProjectionProtocolError::InvalidBatch(format!(
                            "projection model `{}` is already registered for table `{table}`",
                            declaration.model
                        )));
                    }
                    continue;
                }

                let mut table_registration = QueryBuilder::<DB>::new(
                    "SELECT topology_bytes, model_name FROM projection_registered_models \
                     WHERE topology_hash = ",
                );
                table_registration.push_bind(topology_hash.as_slice());
                table_registration.push(" AND table_name = ");
                table_registration.push_bind(declaration.table.as_str());
                if let Some(row) = table_registration
                    .build()
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(|error| {
                        protocol_storage_error::<DB>("load topology table registration", error)
                    })?
                {
                    let bytes: Vec<u8> = row.try_get("topology_bytes").map_err(|error| {
                        protocol_storage_error::<DB>("decode topology table registration", error)
                    })?;
                    verify_bytes(&bytes, &topology_bytes, "registered projector topology")?;
                    let model: String = row.try_get("model_name").map_err(|error| {
                        protocol_storage_error::<DB>("decode topology table registration", error)
                    })?;
                    return Err(ProjectionProtocolError::InvalidBatch(format!(
                        "projection table `{}` is already registered for model `{model}`",
                        declaration.table
                    )));
                }

                let mut insert = QueryBuilder::<DB>::new(
                    "INSERT INTO projection_registered_models \
                     (topology_bytes, topology_hash, model_name, table_name) VALUES (",
                );
                insert.push_bind(topology_bytes.as_slice());
                insert.push(", ");
                insert.push_bind(topology_hash.as_slice());
                insert.push(", ");
                insert.push_bind(declaration.model.as_str());
                insert.push(", ");
                insert.push_bind(declaration.table.as_str());
                insert.push(") ON CONFLICT DO NOTHING");
                let result = insert.build().execute(&mut *tx).await.map_err(|error| {
                    protocol_storage_error::<DB>("insert topology projection registration", error)
                })?;
                if DB::rows_affected(&result) != 1 {
                    let mut verify = QueryBuilder::<DB>::new(
                        "SELECT topology_bytes, table_name FROM projection_registered_models \
                         WHERE topology_hash = ",
                    );
                    verify.push_bind(topology_hash.as_slice());
                    verify.push(" AND model_name = ");
                    verify.push_bind(declaration.model.as_str());
                    let row = verify
                        .build()
                        .fetch_optional(&mut *tx)
                        .await
                        .map_err(|error| {
                            protocol_storage_error::<DB>(
                                "verify concurrent topology projection registration",
                                error,
                            )
                        })?
                        .ok_or_else(|| {
                            ProjectionProtocolError::InvalidBatch(format!(
                                "projection table `{}` was concurrently registered to another model",
                                declaration.table
                            ))
                        })?;
                    let bytes: Vec<u8> = row.try_get("topology_bytes").map_err(|error| {
                        protocol_storage_error::<DB>(
                            "decode concurrent topology projection registration",
                            error,
                        )
                    })?;
                    verify_bytes(&bytes, &topology_bytes, "registered projector topology")?;
                    let table: String = row.try_get("table_name").map_err(|error| {
                        protocol_storage_error::<DB>(
                            "decode concurrent topology projection registration",
                            error,
                        )
                    })?;
                    if table != declaration.table {
                        return Err(ProjectionProtocolError::InvalidBatch(format!(
                            "projection model `{}` was concurrently registered for table `{table}`",
                            declaration.model
                        )));
                    }
                }
            }
            tx.commit().await.map_err(|error| {
                protocol_storage_error::<DB>("commit projection registration", error)
            })
        }
    }

    fn commit_projection(
        &self,
        batch: ProjectionCommitBatch,
    ) -> impl Future<Output = Result<ProjectionCommitResult, ProjectionProtocolError>> + Send + '_
    {
        async move {
            batch.validate()?;
            let write_plan = TableWritePlan::new(
                batch
                    .mutations
                    .iter()
                    .map(|mutation| mutation.mutation.clone())
                    .collect(),
            );
            validate_sql_write_plan(&write_plan)?;

            let topology = batch.input.cursor.topology().clone();
            let partition = batch.input.cursor.projection_partition().clone();
            let mut tx =
                self.pool().begin().await.map_err(|error| {
                    protocol_storage_error::<DB>("begin projection commit", error)
                })?;
            verify_registered_topology_in_tx(&mut tx, &topology).await?;
            let mut state =
                lock_partition_in_tx(&mut tx, &topology, &partition, &batch.change_epoch).await?;
            validate_input_identity_in_tx(&mut tx, &batch.input).await?;
            ensure_active_input(&state, &batch.input)?;
            match classify_validated_input_in_tx(&mut tx, &batch.input, &state).await? {
                InputDisposition::Duplicate(checkpoint) => {
                    return Ok(ProjectionCommitResult::not_applied(
                        ProjectionCommitOutcome::Duplicate,
                        Some(checkpoint),
                    ));
                }
                InputDisposition::Stale(checkpoint) => {
                    return Ok(ProjectionCommitResult::not_applied(
                        ProjectionCommitOutcome::StaleInput,
                        Some(checkpoint),
                    ));
                }
                InputDisposition::New => {
                    ensure_pending_retry_input_in_tx(&mut tx, &state, &batch.input).await?;
                }
            }
            ensure_inbox_available_in_tx(&mut tx, &batch.input).await?;
            ensure_partition_ownership_in_tx(&mut tx, &topology, &partition, &batch.ownership)
                .await?;

            for mutation in &batch.mutations {
                if mutation.mutation.table_name()
                    != batch
                        .ownership
                        .iter()
                        .find(|owned| owned.model == mutation.scope.model())
                        .map(|owned| owned.table.as_str())
                        .unwrap_or_default()
                    || table_model_name(&mutation.mutation) != mutation.scope.model()
                {
                    return Err(ProjectionProtocolError::InvalidBatch(format!(
                        "projection mutation for model `{}` does not target its registered table",
                        mutation.scope.model()
                    )));
                }
            }

            let mut records = Vec::with_capacity(batch.mutations.len());
            let mut records_by_scope = HashMap::with_capacity(batch.mutations.len());
            let mut changes =
                Vec::with_capacity(batch.mutations.len() + batch.observations.len().max(1));
            for mutation in &batch.mutations {
                let current = record_in_tx(&mut tx, &mutation.scope, &state.change_epoch).await?;
                let physical_exists =
                    physical_row_exists_in_tx(&mut tx, &mutation.mutation).await?;
                match current.as_ref().map(|record| &record.metadata) {
                    None if physical_exists => {
                        return Err(ProjectionProtocolError::RecordAlreadyExists {
                            model: mutation.scope.model().to_string(),
                        });
                    }
                    Some(metadata) if metadata.tombstone && physical_exists => {
                        return Err(ProjectionProtocolError::RecordAlreadyExists {
                            model: mutation.scope.model().to_string(),
                        });
                    }
                    Some(metadata) if !metadata.tombstone && !physical_exists => {
                        return Err(ProjectionProtocolError::RecordMissing {
                            model: mutation.scope.model().to_string(),
                        });
                    }
                    _ => {}
                }
                let (revision, tombstone) = next_record(
                    &mutation.scope,
                    &mutation.expectation,
                    mutation.kind,
                    current.as_ref(),
                )?;
                let change = allocate_change(
                    &mut state,
                    &topology,
                    &partition,
                    change_kind_for_mutation(mutation.kind),
                    batch.input.causation_id.clone(),
                    None,
                    Some(mutation.scope.clone()),
                    Some(revision.clone()),
                    None,
                )?;
                let metadata = ProjectionRecordMetadata {
                    revision,
                    tombstone,
                    change: change.cursor.clone(),
                };
                records_by_scope.insert(mutation.scope.clone(), metadata.clone());
                records.push(metadata);
                changes.push(change);
            }

            let mut observations = Vec::with_capacity(batch.observations.len());
            for request in &batch.observations {
                let (scope, revision, staged_change) = match &request.target {
                    ProjectionObservationTarget::StagedRecord(scope) => {
                        let metadata = records_by_scope.get(scope).ok_or_else(|| {
                            ProjectionProtocolError::InvalidBatch(format!(
                                "projection observation references unstaged model `{}`",
                                scope.model()
                            ))
                        })?;
                        (
                            scope.clone(),
                            Some(metadata.revision.clone()),
                            Some(metadata.change.clone()),
                        )
                    }
                    ProjectionObservationTarget::ExistingRecord(expected) => {
                        let metadata =
                            if let Some(metadata) = records_by_scope.get(expected.scope()) {
                                metadata.clone()
                            } else {
                                record_in_tx(&mut tx, expected.scope(), &state.change_epoch)
                                    .await?
                                    .map(|record| record.metadata)
                                    .ok_or_else(|| ProjectionProtocolError::RecordMissing {
                                        model: expected.scope().model().to_string(),
                                    })?
                            };
                        if metadata.revision != *expected {
                            return Err(ProjectionProtocolError::RecordRevisionConflict {
                                model: expected.scope().model().to_string(),
                                expected_incarnation: expected.incarnation(),
                                expected_revision: expected.revision(),
                                actual_incarnation: metadata.revision.incarnation(),
                                actual_revision: metadata.revision.revision(),
                            });
                        }
                        if metadata.tombstone {
                            return Err(ProjectionProtocolError::RecordTombstoned {
                                model: expected.scope().model().to_string(),
                            });
                        }
                        (expected.scope().clone(), Some(expected.clone()), None)
                    }
                    ProjectionObservationTarget::Dependency(scope) => (scope.clone(), None, None),
                };
                if observation_in_tx(
                    &mut tx,
                    &batch.input.causation_id,
                    &scope,
                    request.kind,
                    &state.change_epoch,
                )
                .await?
                .is_some()
                {
                    // Causation observations are immutable earliest evidence.
                    // A later input carrying the same exact observation key
                    // neither rewrites its revision nor emits a duplicate
                    // change.
                    continue;
                }

                let change_cursor = if let Some(change) = staged_change {
                    change
                } else {
                    let change = allocate_change(
                        &mut state,
                        &topology,
                        &partition,
                        ProjectionChangeKind::Observation,
                        batch.input.causation_id.clone(),
                        Some(request.kind),
                        Some(scope.clone()),
                        revision.clone(),
                        None,
                    )?;
                    let cursor = change.cursor.clone();
                    changes.push(change);
                    cursor
                };
                observations.push(ProjectionObservation {
                    causation_id: batch.input.causation_id.clone(),
                    kind: request.kind,
                    revision,
                    scope,
                    change: change_cursor,
                });
            }

            if changes.is_empty() {
                changes.push(allocate_change(
                    &mut state,
                    &topology,
                    &partition,
                    ProjectionChangeKind::Checkpoint,
                    batch.input.causation_id.clone(),
                    None,
                    None,
                    None,
                    None,
                )?);
            }
            let final_change = changes
                .last()
                .expect("a successful projection commit always allocates a change")
                .cursor
                .clone();
            let checkpoint = ProjectionCheckpoint::new(
                batch.input.cursor.clone(),
                final_change.clone(),
                batch.input.gap_free,
            )?;

            apply_read_model_write_plan_in_tx(&mut tx, write_plan).await?;
            for change in &changes {
                insert_change_in_tx(&mut tx, change).await?;
            }
            for metadata in &records {
                upsert_record_in_tx(&mut tx, metadata).await?;
            }
            for observation in &observations {
                insert_observation_in_tx(&mut tx, observation).await?;
            }
            store_input_cursor_in_tx(&mut tx, &batch.input, &final_change).await?;
            insert_input_identity_in_tx(&mut tx, &batch.input).await?;
            insert_input_receipt_in_tx(&mut tx, &batch.input, "applied", None, &final_change)
                .await?;
            insert_inbox_in_tx(&mut tx, &batch.input).await?;
            update_partition_head_in_tx(
                &mut tx,
                &topology,
                &partition,
                state.change_head,
                state.pending_retry_failure_id.as_deref(),
            )
            .await?;
            retain_projection_change_suffix_in_tx(
                &mut tx,
                &topology,
                &partition,
                &state,
                self.projection_change_retention(),
            )
            .await?;

            let mut changed_tables = batch
                .mutations
                .iter()
                .map(|mutation| mutation.mutation.table_name().to_string())
                .collect::<BTreeSet<_>>();
            changed_tables.insert(PROJECTION_CHANGE_NOTIFY_TABLE.to_string());
            if self.projection_notify_enabled() {
                DB::push_change_notify(&mut *tx, &changed_tables).await?;
            }
            tx.commit().await.map_err(|error| {
                protocol_storage_error::<DB>("commit projection transaction", error)
            })?;
            self.publish_read_model_change(crate::ReadModelChange {
                tables: changed_tables,
            });
            Ok(ProjectionCommitResult {
                outcome: ProjectionCommitOutcome::Applied,
                checkpoint: Some(checkpoint),
                records,
                changes,
            })
        }
    }

    fn record_projection_failure(
        &self,
        batch: ProjectionFailureBatch,
    ) -> impl Future<Output = Result<ProjectionFailure, ProjectionProtocolError>> + Send + '_ {
        async move {
            batch.validate()?;
            let topology = batch.input.cursor.topology().clone();
            let partition = batch.input.cursor.projection_partition().clone();
            let mut tx =
                self.pool().begin().await.map_err(|error| {
                    protocol_storage_error::<DB>("begin projection failure", error)
                })?;
            verify_registered_topology_in_tx(&mut tx, &topology).await?;
            let mut state =
                lock_partition_in_tx(&mut tx, &topology, &partition, &batch.change_epoch).await?;
            validate_input_identity_in_tx(&mut tx, &batch.input).await?;
            if state.active_generation != batch.input.generation {
                return Err(ProjectionProtocolError::GenerationFenced {
                    expected: state.active_generation.get(),
                    actual: batch.input.generation.get(),
                });
            }
            if let Some(stopped_failure_id) = &state.stopped_failure_id {
                if stopped_failure_id == &batch.failure_id {
                    let existing = failure_in_tx(
                        &mut tx,
                        &topology,
                        &partition,
                        stopped_failure_id,
                        &state.change_epoch,
                    )
                    .await?
                    .ok_or_else(|| {
                        corrupt_storage(format!(
                            "stopped projection failure `{stopped_failure_id}` is missing"
                        ))
                    })?;
                    if failure_matches_batch(&existing, &batch) {
                        return Ok(existing.failure);
                    }
                    if existing.failure.input == batch.input.cursor {
                        if existing.failure.input_fingerprint != batch.input.fingerprint
                            || existing.failure.message_id != batch.input.message_id
                            || existing.failure.causation_id != batch.input.causation_id
                            || existing.failure.gap_free != batch.input.gap_free
                        {
                            return Err(ProjectionProtocolError::InputCorruption);
                        }
                    } else if existing.failure.message_id == batch.input.message_id {
                        return Err(ProjectionProtocolError::MessageIdReuse {
                            message_id: batch.input.message_id.clone(),
                        });
                    }
                }
                return Err(ProjectionProtocolError::PartitionStopped {
                    failure_id: stopped_failure_id.clone(),
                });
            }
            let mut failure_id_query =
                QueryBuilder::<DB>::new("SELECT 1 FROM projection_failures WHERE failure_id = ");
            failure_id_query.push_bind(batch.failure_id.as_str());
            failure_id_query.push(" LIMIT 1");
            if failure_id_query
                .build()
                .fetch_optional(&mut *tx)
                .await
                .map_err(|error| {
                    protocol_storage_error::<DB>("check projection failure ID", error)
                })?
                .is_some()
            {
                return Err(ProjectionProtocolError::InvalidBatch(format!(
                    "projection failure ID `{}` is already bound to another failure",
                    batch.failure_id
                )));
            }

            match classify_validated_input_in_tx(&mut tx, &batch.input, &state).await? {
                InputDisposition::New => {
                    ensure_pending_retry_input_in_tx(&mut tx, &state, &batch.input).await?;
                }
                InputDisposition::Duplicate(_) | InputDisposition::Stale(_) => {
                    return Err(ProjectionProtocolError::InvalidBatch(
                        "cannot record terminal failure for an already processed input".into(),
                    ));
                }
            }
            ensure_inbox_available_in_tx(&mut tx, &batch.input).await?;
            let change = allocate_change(
                &mut state,
                &topology,
                &partition,
                ProjectionChangeKind::Failure,
                batch.input.causation_id.clone(),
                None,
                None,
                None,
                Some(batch.failure_id.clone()),
            )?;
            insert_change_in_tx(&mut tx, &change).await?;
            insert_failure_in_tx(&mut tx, &batch, &change.cursor).await?;
            insert_input_identity_in_tx(&mut tx, &batch.input).await?;
            insert_input_receipt_in_tx(
                &mut tx,
                &batch.input,
                "failed",
                Some(&batch.failure_id),
                &change.cursor,
            )
            .await?;
            insert_inbox_in_tx(&mut tx, &batch.input).await?;
            stop_partition_in_tx(&mut tx, &batch, state.change_head).await?;
            retain_projection_change_suffix_in_tx(
                &mut tx,
                &topology,
                &partition,
                &state,
                self.projection_change_retention(),
            )
            .await?;

            let changed_tables = BTreeSet::from([PROJECTION_CHANGE_NOTIFY_TABLE.to_string()]);
            if self.projection_notify_enabled() {
                DB::push_change_notify(&mut *tx, &changed_tables).await?;
            }
            let failure = ProjectionFailure {
                failure_id: batch.failure_id,
                input: batch.input.cursor,
                input_fingerprint: batch.input.fingerprint,
                message_id: batch.input.message_id,
                causation_id: batch.input.causation_id,
                generation: batch.input.generation,
                gap_free: batch.input.gap_free,
                failure_code: batch.failure_code,
                failure_bytes: batch.failure_bytes,
                failure_digest: batch.failure_digest,
                change: change.cursor,
            };
            tx.commit().await.map_err(|error| {
                protocol_storage_error::<DB>("commit projection failure", error)
            })?;
            self.publish_read_model_change(crate::ReadModelChange {
                tables: changed_tables,
            });
            Ok(failure)
        }
    }

    fn projection_checkpoint<'a>(
        &'a self,
        cursor_scope: &'a ProjectionInputCursor,
        generation: ProjectionGeneration,
    ) -> impl Future<Output = Result<Option<ProjectionCheckpoint>, ProjectionProtocolError>> + Send + 'a
    {
        async move {
            let Some(state) = load_partition(
                self.pool(),
                cursor_scope.topology(),
                cursor_scope.projection_partition(),
            )
            .await?
            else {
                return Ok(None);
            };
            let probe = TrustedProjectionInput {
                cursor: cursor_scope.clone(),
                fingerprint: ProjectionInputFingerprint::from_digest([0; 32]),
                message_id: String::new(),
                causation_id: String::new(),
                generation,
                gap_free: false,
            };
            let mut tx = self.pool().begin().await.map_err(|error| {
                protocol_storage_error::<DB>("begin projection checkpoint read", error)
            })?;
            let Some(stored) = current_input_cursor_in_tx(&mut tx, &probe).await? else {
                return Ok(None);
            };
            verify_stored_change(&state, &stored.change)?;
            if stored.source_epoch != *cursor_scope.epoch() {
                return Err(ProjectionProtocolError::IncomparableInput);
            }
            Ok(Some(checkpoint_from_stored(
                cursor_scope,
                stored.source_epoch,
                stored.source_position,
                stored.change,
                stored.gap_free,
            )?))
        }
    }

    fn projection_record<'a>(
        &'a self,
        scope: &'a ProjectionRecordScope,
    ) -> impl Future<Output = Result<Option<ProjectionRecordMetadata>, ProjectionProtocolError>>
           + Send
           + 'a {
        async move {
            let Some(state) =
                load_partition(self.pool(), scope.topology(), scope.projection_partition()).await?
            else {
                return Ok(None);
            };
            let mut tx = self.pool().begin().await.map_err(|error| {
                protocol_storage_error::<DB>("begin projection record read", error)
            })?;
            Ok(record_in_tx(&mut tx, scope, &state.change_epoch)
                .await?
                .map(|record| record.metadata))
        }
    }

    fn projection_input_disposition<'a>(
        &'a self,
        input: &'a TrustedProjectionInput,
    ) -> impl Future<Output = Result<ProjectionInputDisposition, ProjectionProtocolError>> + Send + 'a
    {
        async move {
            let mut tx = self.pool().begin().await.map_err(|error| {
                protocol_storage_error::<DB>("begin projection input disposition read", error)
            })?;
            if DB::BACKEND == "postgres" {
                sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
                    .execute(&mut *tx)
                    .await
                    .map_err(|error| {
                        protocol_storage_error::<DB>(
                            "configure projection input disposition read",
                            error,
                        )
                    })?;
            }

            let result = async {
                verify_registered_topology_in_tx(&mut tx, input.cursor.topology()).await?;
                // Durable cursor/message/capability corruption wins over a
                // generation fence, matching commit/failure validation.
                validate_input_identity_read_only_in_tx(&mut tx, input).await?;
                let Some(state) = load_partition_in_tx(
                    &mut tx,
                    input.cursor.topology(),
                    input.cursor.projection_partition(),
                )
                .await?
                else {
                    if input.generation != ProjectionGeneration::initial() {
                        return Err(ProjectionProtocolError::GenerationFenced {
                            expected: ProjectionGeneration::initial().get(),
                            actual: input.generation.get(),
                        });
                    }
                    return Ok(ProjectionInputDisposition::Pending);
                };
                if state.active_generation != input.generation {
                    return Err(ProjectionProtocolError::GenerationFenced {
                        expected: state.active_generation.get(),
                        actual: input.generation.get(),
                    });
                }
                verify_generation_exists_in_tx(
                    &mut tx,
                    input.cursor.topology(),
                    input.cursor.projection_partition(),
                    input.generation,
                )
                .await?;
                if let Some(failure_id) = &state.stopped_failure_id {
                    return Err(ProjectionProtocolError::PartitionStopped {
                        failure_id: failure_id.clone(),
                    });
                }
                match classify_validated_input_in_tx(&mut tx, input, &state).await? {
                    InputDisposition::New => {
                        ensure_pending_retry_input_in_tx(&mut tx, &state, input).await?;
                        Ok(ProjectionInputDisposition::Pending)
                    }
                    InputDisposition::Duplicate(checkpoint) => {
                        Ok(ProjectionInputDisposition::Duplicate(checkpoint))
                    }
                    InputDisposition::Stale(checkpoint) => {
                        Ok(ProjectionInputDisposition::Stale(checkpoint))
                    }
                }
            }
            .await;

            match result {
                Ok(disposition) => {
                    tx.commit().await.map_err(|error| {
                        protocol_storage_error::<DB>(
                            "commit projection input disposition read",
                            error,
                        )
                    })?;
                    Ok(disposition)
                }
                Err(error) => {
                    tx.rollback().await.map_err(|rollback_error| {
                        protocol_storage_error::<DB>(
                            "roll back failed projection input disposition read",
                            rollback_error,
                        )
                    })?;
                    Err(error)
                }
            }
        }
    }

    fn projection_query_snapshot<'a>(
        &'a self,
        request: &'a ProjectionQuerySnapshotRequest,
    ) -> impl Future<Output = Result<ProjectionQuerySnapshot, ProjectionProtocolError>> + Send + 'a
    {
        read_projection_query_snapshot_in_executor(self.pool(), request)
    }

    fn projection_query_snapshot_batch<'a>(
        &'a self,
        request: &'a ProjectionQuerySnapshotBatchRequest,
    ) -> impl Future<Output = Result<ProjectionQuerySnapshotBatch, ProjectionProtocolError>> + Send + 'a
    {
        let request = request.clone();
        async move {
            request.validate()?;
            with_projection_read_snapshot(self.pool(), move |connection| {
                Box::pin(async move {
                    let mut snapshots = Vec::with_capacity(request.requests.len());
                    for row_request in &request.requests {
                        snapshots.push(
                            read_projection_query_snapshot_in_executor::<DB, _>(
                                &mut *connection,
                                row_request,
                            )
                            .await?,
                        );
                    }
                    Ok(ProjectionQuerySnapshotBatch { snapshots })
                })
            })
            .await
        }
    }

    fn projection_obligation_evidence_batch<'a>(
        &'a self,
        request: &'a ProjectionObligationEvidenceBatchRequest,
    ) -> impl Future<Output = Result<ProjectionObligationEvidenceBatch, ProjectionProtocolError>>
           + Send
           + 'a {
        let request = request.clone();
        async move {
            request.validate()?;
            with_projection_read_snapshot(self.pool(), move |connection| {
                Box::pin(async move {
                    read_projection_obligation_evidence_batch_in_executor::<DB>(
                        connection, &request,
                    )
                    .await
                })
            })
            .await
        }
    }

    fn projection_live_record_batch<'a>(
        &'a self,
        request: &'a ProjectionLiveRecordBatchRequest,
    ) -> impl Future<Output = Result<ProjectionLiveRecordBatch, ProjectionProtocolError>> + Send + 'a
    {
        let request = request.clone();
        async move {
            request.validate()?;
            with_projection_read_snapshot(self.pool(), move |connection| {
                Box::pin(async move {
                    read_projection_live_record_batch_in_executor::<DB>(connection, &request).await
                })
            })
            .await
        }
    }

    fn projection_partition_runtime_state<'a>(
        &'a self,
        topology: &'a ProjectorTopologyId,
        partition: &'a ProjectionPartition,
    ) -> impl Future<Output = Result<Option<ProjectionPartitionRuntimeState>, ProjectionProtocolError>>
           + Send
           + 'a {
        load_partition_runtime_state(self.pool(), topology, partition)
    }

    fn projection_observation<'a>(
        &'a self,
        causation_id: &'a str,
        scope: &'a ProjectionRecordScope,
        kind: ProjectionObservationKind,
    ) -> impl Future<Output = Result<Option<ProjectionObservation>, ProjectionProtocolError>> + Send + 'a
    {
        async move {
            let Some(state) =
                load_partition(self.pool(), scope.topology(), scope.projection_partition()).await?
            else {
                return Ok(None);
            };
            let mut tx = self.pool().begin().await.map_err(|error| {
                protocol_storage_error::<DB>("begin projection observation read", error)
            })?;
            observation_in_tx(&mut tx, causation_id, scope, kind, &state.change_epoch).await
        }
    }

    fn projection_changes<'a>(
        &'a self,
        topology: &'a ProjectorTopologyId,
        partition: &'a ProjectionPartition,
        after: Option<&'a ProjectionChangeCursor>,
        limit: usize,
    ) -> impl Future<Output = Result<ProjectionChangeRead, ProjectionProtocolError>> + Send + 'a
    {
        let topology = topology.clone();
        let partition = partition.clone();
        let after = after.cloned();
        async move {
            read_projection_changes_in_snapshot(
                self.pool(),
                topology,
                partition,
                after,
                limit,
                std::future::ready(()),
            )
            .await
        }
    }

    fn repair_projection<'a>(
        &'a self,
        topology: &'a ProjectorTopologyId,
        partition: &'a ProjectionPartition,
        failure_id: &'a str,
    ) -> impl Future<Output = Result<ProjectionGeneration, ProjectionProtocolError>> + Send + 'a
    {
        async move {
            let mut tx =
                self.pool().begin().await.map_err(|error| {
                    protocol_storage_error::<DB>("begin projection repair", error)
                })?;
            let Some(state) = lock_existing_partition_in_tx(&mut tx, topology, partition).await?
            else {
                return Err(ProjectionProtocolError::InvalidBatch(
                    "cannot repair an unknown projection partition".into(),
                ));
            };
            match &state.stopped_failure_id {
                Some(stopped) if stopped == failure_id => {}
                Some(stopped) => {
                    return Err(ProjectionProtocolError::PartitionStopped {
                        failure_id: stopped.clone(),
                    });
                }
                None => {
                    return Err(ProjectionProtocolError::InvalidBatch(
                        "projection partition is not stopped".into(),
                    ));
                }
            }
            verify_generation_exists_in_tx(&mut tx, topology, partition, state.active_generation)
                .await?;
            let failure = failure_in_tx(
                &mut tx,
                topology,
                partition,
                failure_id,
                &state.change_epoch,
            )
            .await?
            .ok_or_else(|| {
                corrupt_storage(format!(
                    "stopped projection failure `{failure_id}` is missing"
                ))
            })?;
            if failure.failure.generation != state.active_generation {
                return Err(corrupt_storage(format!(
                    "stopped failure generation {} differs from active generation {}",
                    failure.failure.generation.get(),
                    state.active_generation.get()
                )));
            }
            let next_generation = state.active_generation.checked_next()?;
            let topology_hash = topology.digest();
            let partition_hash = partition.digest();
            let old_generation =
                to_i64::<DB>(state.active_generation.get(), "projection generation")?;
            let next_generation_value =
                to_i64::<DB>(next_generation.get(), "projection generation")?;

            let mut existing = QueryBuilder::<DB>::new(
                "SELECT 1 FROM projection_generations WHERE topology_hash = ",
            );
            existing.push_bind(topology_hash.as_slice());
            existing.push(" AND partition_hash = ");
            existing.push_bind(partition_hash.as_slice());
            existing.push(" AND generation = ");
            existing.push_bind(next_generation_value);
            existing.push(" LIMIT 1");
            if existing
                .build()
                .fetch_optional(&mut *tx)
                .await
                .map_err(|error| {
                    protocol_storage_error::<DB>("check projection repair generation", error)
                })?
                .is_some()
            {
                return Err(corrupt_storage(format!(
                    "projection repair generation {} already exists",
                    next_generation.get()
                )));
            }
            let mut retry_link = QueryBuilder::<DB>::new(
                "SELECT generation FROM projection_generations WHERE topology_hash = ",
            );
            retry_link.push_bind(topology_hash.as_slice());
            retry_link.push(" AND partition_hash = ");
            retry_link.push_bind(partition_hash.as_slice());
            retry_link.push(" AND retry_of_failure_id = ");
            retry_link.push_bind(failure_id);
            retry_link.push(" LIMIT 1");
            if retry_link
                .build()
                .fetch_optional(&mut *tx)
                .await
                .map_err(|error| {
                    protocol_storage_error::<DB>("check projection repair failure link", error)
                })?
                .is_some()
            {
                return Err(corrupt_storage(format!(
                    "stopped failure `{failure_id}` already has a repair generation"
                )));
            }

            let mut insert_generation = QueryBuilder::<DB>::new(
                "INSERT INTO projection_generations \
                 (topology_hash, partition_hash, generation, retry_of_generation, \
                 retry_of_failure_id) VALUES (",
            );
            insert_generation.push_bind(topology_hash.as_slice());
            insert_generation.push(", ");
            insert_generation.push_bind(partition_hash.as_slice());
            insert_generation.push(", ");
            insert_generation.push_bind(next_generation_value);
            insert_generation.push(", ");
            insert_generation.push_bind(old_generation);
            insert_generation.push(", ");
            insert_generation.push_bind(failure_id);
            insert_generation.push(")");
            insert_generation
                .build()
                .execute(&mut *tx)
                .await
                .map_err(|error| {
                    protocol_storage_error::<DB>("insert projection repair generation", error)
                })?;

            let mut copy = QueryBuilder::<DB>::new(
                "INSERT INTO projection_input_cursors \
                 (topology_hash, partition_hash, source_bytes, source_hash, source_partition_bytes, \
                 source_partition_hash, source_epoch, source_position, input_hash, message_id, \
                 causation_id, gap_free, generation, change_epoch, change_position) \
                 SELECT topology_hash, partition_hash, source_bytes, source_hash, \
                 source_partition_bytes, source_partition_hash, source_epoch, source_position, \
                 input_hash, message_id, causation_id, gap_free, ",
            );
            copy.push_bind(next_generation_value);
            copy.push(
                ", change_epoch, change_position FROM projection_input_cursors \
                 WHERE topology_hash = ",
            );
            copy.push_bind(topology_hash.as_slice());
            copy.push(" AND partition_hash = ");
            copy.push_bind(partition_hash.as_slice());
            copy.push(" AND generation = ");
            copy.push_bind(old_generation);
            copy.build().execute(&mut *tx).await.map_err(|error| {
                protocol_storage_error::<DB>("copy projection repair checkpoints", error)
            })?;

            let mut activate =
                QueryBuilder::<DB>::new("UPDATE projection_partitions SET active_generation = ");
            activate.push_bind(next_generation_value);
            activate.push(", pending_retry_failure_id = ");
            activate.push_bind(failure_id);
            activate.push(
                ", stopped_failure_id = NULL, stopped_source_bytes = NULL, \
                 stopped_source_hash = NULL, stopped_source_partition_bytes = NULL, \
                 stopped_source_partition_hash = NULL, stopped_source_epoch = NULL, \
                 stopped_source_position = NULL, stopped_generation = NULL, \
                 stopped_input_hash = NULL, stopped_message_id = NULL, \
                 stopped_causation_id = NULL, stopped_gap_free = NULL WHERE topology_hash = ",
            );
            activate.push_bind(topology_hash.as_slice());
            activate.push(" AND partition_hash = ");
            activate.push_bind(partition_hash.as_slice());
            activate.push(" AND active_generation = ");
            activate.push_bind(old_generation);
            activate.push(" AND stopped_failure_id = ");
            activate.push_bind(failure_id);
            let result = activate.build().execute(&mut *tx).await.map_err(|error| {
                protocol_storage_error::<DB>("activate projection repair generation", error)
            })?;
            if DB::rows_affected(&result) != 1 {
                return Err(corrupt_storage(
                    "projection stop fence changed while its partition lock was held",
                ));
            }
            tx.commit()
                .await
                .map_err(|error| protocol_storage_error::<DB>("commit projection repair", error))?;
            Ok(next_generation)
        }
    }

    fn compact_projection_changes<'a>(
        &'a self,
        through: &'a ProjectionChangeCursor,
    ) -> impl Future<Output = Result<u64, ProjectionProtocolError>> + Send + 'a {
        async move {
            let topology = through.topology();
            let partition = through.projection_partition();
            let mut tx = self.pool().begin().await.map_err(|error| {
                protocol_storage_error::<DB>("begin projection change compaction", error)
            })?;
            let Some(state) = lock_existing_partition_in_tx(&mut tx, topology, partition).await?
            else {
                return Err(ProjectionProtocolError::InvalidBatch(
                    "cannot compact an unknown projection partition".into(),
                ));
            };
            if through.epoch() != &state.change_epoch {
                return Err(ProjectionProtocolError::ScopeMismatch {
                    field: "projection compaction epoch",
                });
            }
            if through.position() > state.change_head {
                return Err(ProjectionProtocolError::InvalidBatch(
                    "cannot compact beyond the projection change head".into(),
                ));
            }
            if through.position() <= state.compacted_through {
                return Ok(state.compacted_through);
            }
            let topology_hash = topology.digest();
            let partition_hash = partition.digest();
            let through_position =
                to_i64::<DB>(through.position(), "projection compaction position")?;
            let mut exact =
                QueryBuilder::<DB>::new("SELECT 1 FROM projection_changes WHERE topology_hash = ");
            exact.push_bind(topology_hash.as_slice());
            exact.push(" AND partition_hash = ");
            exact.push_bind(partition_hash.as_slice());
            exact.push(" AND change_epoch = ");
            exact.push_bind(state.change_epoch.as_str());
            exact.push(" AND change_position = ");
            exact.push_bind(through_position);
            exact.push(" LIMIT 1");
            if exact
                .build()
                .fetch_optional(&mut *tx)
                .await
                .map_err(|error| {
                    protocol_storage_error::<DB>("verify projection compaction cursor", error)
                })?
                .is_none()
            {
                return Err(corrupt_storage(format!(
                    "projection compaction cursor {} is missing",
                    through.position()
                )));
            }

            let mut delete =
                QueryBuilder::<DB>::new("DELETE FROM projection_changes WHERE topology_hash = ");
            delete.push_bind(topology_hash.as_slice());
            delete.push(" AND partition_hash = ");
            delete.push_bind(partition_hash.as_slice());
            delete.push(" AND change_epoch = ");
            delete.push_bind(state.change_epoch.as_str());
            delete.push(" AND change_position <= ");
            delete.push_bind(through_position);
            let result = delete.build().execute(&mut *tx).await.map_err(|error| {
                protocol_storage_error::<DB>("compact projection changes", error)
            })?;
            let expected_removed = through.position() - state.compacted_through;
            if DB::rows_affected(&result) != expected_removed {
                return Err(corrupt_storage(format!(
                    "projection compaction expected to remove {expected_removed} changes but removed {}",
                    DB::rows_affected(&result)
                )));
            }

            let mut watermark =
                QueryBuilder::<DB>::new("UPDATE projection_partitions SET compacted_through = ");
            watermark.push_bind(through_position);
            watermark.push(" WHERE topology_hash = ");
            watermark.push_bind(topology_hash.as_slice());
            watermark.push(" AND partition_hash = ");
            watermark.push_bind(partition_hash.as_slice());
            let result = watermark.build().execute(&mut *tx).await.map_err(|error| {
                protocol_storage_error::<DB>("advance projection compaction watermark", error)
            })?;
            if DB::rows_affected(&result) != 1 {
                return Err(corrupt_storage(
                    "projection partition disappeared during compaction",
                ));
            }
            tx.commit().await.map_err(|error| {
                protocol_storage_error::<DB>("commit projection change compaction", error)
            })?;
            Ok(through.position())
        }
    }

    fn projection_failure<'a>(
        &'a self,
        topology: &'a ProjectorTopologyId,
        partition: &'a ProjectionPartition,
        failure_id: &'a str,
    ) -> impl Future<Output = Result<Option<ProjectionFailure>, ProjectionProtocolError>> + Send + 'a
    {
        async move {
            let Some(state) = load_partition(self.pool(), topology, partition).await? else {
                return Ok(None);
            };
            let mut tx = self.pool().begin().await.map_err(|error| {
                protocol_storage_error::<DB>("begin projection failure read", error)
            })?;
            Ok(failure_in_tx(
                &mut tx,
                topology,
                partition,
                failure_id,
                &state.change_epoch,
            )
            .await?
            .map(|stored| stored.failure))
        }
    }

    fn projection_failure_location<'a>(
        &'a self,
        failure_id: &'a str,
    ) -> impl Future<Output = Result<Option<ProjectionFailureLocation>, ProjectionProtocolError>>
           + Send
           + 'a {
        async move {
            let mut builder = QueryBuilder::<DB>::new(
                "SELECT partition.topology_bytes, partition.topology_hash, \
                 partition.partition_bytes, partition.partition_hash \
                 FROM projection_failures failure \
                 INNER JOIN projection_partitions partition \
                 ON partition.topology_hash = failure.topology_hash \
                 AND partition.partition_hash = failure.partition_hash \
                 WHERE failure.failure_id = ",
            );
            builder.push_bind(failure_id);
            let Some(row) = builder
                .build()
                .fetch_optional(self.pool())
                .await
                .map_err(|error| {
                    protocol_storage_error::<DB>("resolve projection failure location", error)
                })?
            else {
                return Ok(None);
            };

            let topology_bytes: Vec<u8> = row.try_get("topology_bytes").map_err(|error| {
                protocol_storage_error::<DB>("decode repair topology bytes", error)
            })?;
            let topology = ProjectorTopologyId::from_canonical_bytes(&topology_bytes)?;
            let topology_hash: Vec<u8> = row.try_get("topology_hash").map_err(|error| {
                protocol_storage_error::<DB>("decode repair topology hash", error)
            })?;
            verify_digest(
                &topology_hash,
                topology.digest(),
                "projection repair topology",
            )?;

            let partition_bytes: Vec<u8> = row.try_get("partition_bytes").map_err(|error| {
                protocol_storage_error::<DB>("decode repair partition bytes", error)
            })?;
            let partition = ProjectionPartition::new(partition_bytes)?;
            let partition_hash: Vec<u8> = row.try_get("partition_hash").map_err(|error| {
                protocol_storage_error::<DB>("decode repair partition hash", error)
            })?;
            verify_digest(
                &partition_hash,
                partition.digest(),
                "projection repair partition",
            )?;

            Ok(Some(ProjectionFailureLocation {
                topology,
                partition,
            }))
        }
    }
}

#[cfg(all(test, feature = "sqlite"))]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::LazyLock;
    use std::time::Duration;

    use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
    use sqlx::Row;

    use super::*;
    use crate::command_ledger::{
        CanonicalInputHash, CausalCommitBatch, CausalTransactionalCommit,
        CommandContractFingerprint, CommandId, CommandLedgerError, CommandLedgerKey,
        CommandLedgerStore, CommandLookup, CommandLookupScope, CommandReservation,
        PrincipalPartitionId, ReservationOutcome, TerminalCommandState,
    };
    use crate::projection_protocol::{
        ProjectionCheckpointProbe, ProjectionObservationRequest, ProjectionQuerySnapshotRequest,
        ProjectionRecordMutation, ProjectionScopeCodec,
    };
    use crate::repository::{CommitBatch, ReadModelWritePlanStore, TransactionalCommit};
    use crate::table::{
        ColumnType, DeleteTableRowMutation, ExpectedVersion, PrimaryKey, RowKey, RowValue,
        RowValues, RowWriteMode, TableColumn, TableKind, TableRowMutation, TableSchema,
        TableSchemaRegistry,
    };

    fn topology() -> ProjectorTopologyId {
        ProjectorTopologyId::new(1, "sql_todo_projector", [17; 32]).unwrap()
    }

    fn partition() -> ProjectionPartition {
        ProjectionScopeCodec::new(topology())
            .encode_partition(Some(&serde_json::json!("tenant-sql")))
            .unwrap()
    }

    fn source(name: &str, key: &[u8]) -> ProjectionSource {
        ProjectionSource::new(name, key.to_vec()).unwrap()
    }

    fn input_cursor_for(
        source: ProjectionSource,
        position: u64,
        source_epoch: &str,
    ) -> ProjectionInputCursor {
        ProjectionInputCursor::new(
            topology(),
            partition(),
            source,
            ProjectionEpoch::new(source_epoch).unwrap(),
            position,
        )
        .unwrap()
    }

    fn input_cursor(position: u64) -> ProjectionInputCursor {
        input_cursor_for(source("todo_stream", b"todo-1"), position, "source-v1")
    }

    fn input(
        position: u64,
        fingerprint: &[u8],
        message_id: &str,
        causation_id: &str,
        generation: ProjectionGeneration,
    ) -> TrustedProjectionInput {
        TrustedProjectionInput::mint(
            input_cursor(position),
            ProjectionInputFingerprint::from_canonical_bytes(fingerprint),
            message_id,
            causation_id,
            generation,
            true,
        )
        .unwrap()
    }

    fn non_gap_input(
        position: u64,
        fingerprint: &[u8],
        message_id: &str,
        causation_id: &str,
        generation: ProjectionGeneration,
    ) -> TrustedProjectionInput {
        TrustedProjectionInput::mint(
            input_cursor(position),
            ProjectionInputFingerprint::from_canonical_bytes(fingerprint),
            message_id,
            causation_id,
            generation,
            false,
        )
        .unwrap()
    }

    fn input_for_source(
        source: ProjectionSource,
        position: u64,
        fingerprint: &[u8],
        message_id: &str,
        causation_id: &str,
    ) -> TrustedProjectionInput {
        TrustedProjectionInput::mint(
            input_cursor_for(source, position, "source-v1"),
            ProjectionInputFingerprint::from_canonical_bytes(fingerprint),
            message_id,
            causation_id,
            ProjectionGeneration::initial(),
            true,
        )
        .unwrap()
    }

    fn change_epoch() -> ProjectionEpoch {
        ProjectionEpoch::new("changes-v1").unwrap()
    }

    fn schema() -> &'static TableSchema {
        static SCHEMA: LazyLock<TableSchema> = LazyLock::new(|| TableSchema {
            model_name: "SqlTodoView".into(),
            table_name: "sql_todo_views".into(),
            columns: vec![
                TableColumn {
                    primary_key: true,
                    ..TableColumn::new("id", "id", ColumnType::Text)
                },
                TableColumn {
                    nullable: true,
                    ..TableColumn::new("value", "value", ColumnType::Text)
                },
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

    fn scope_codec() -> ProjectionScopeCodec {
        ProjectionScopeCodec::with_models(topology(), [("SqlTodoView", schema())]).unwrap()
    }

    fn record_key() -> RowKey {
        RowKey::new([("id", RowValue::String("todo-1".into()))])
    }

    fn record_scope() -> ProjectionRecordScope {
        scope_codec()
            .encode_row_scope_in_partition("SqlTodoView", partition(), &record_key())
            .unwrap()
    }

    fn ownership() -> ProjectionModelOwnership {
        ProjectionModelOwnership::new("SqlTodoView", "sql_todo_views").unwrap()
    }

    async fn repository() -> SqlxRepository<sqlx::Sqlite> {
        let repository = unregistered_repository().await;
        repository
            .register_projection_models(&topology(), &[ownership()])
            .await
            .unwrap();
        repository
    }

    async fn repository_with_retention(max_retained_changes: u64) -> SqlxRepository<sqlx::Sqlite> {
        let repository = unregistered_repository()
            .await
            .with_projection_change_retention(
                ProjectionChangeRetention::new(max_retained_changes).unwrap(),
            );
        repository
            .register_projection_models(&topology(), &[ownership()])
            .await
            .unwrap();
        repository
    }

    async fn wal_repository_with_retention(
        max_retained_changes: u64,
    ) -> (SqlxRepository<sqlx::Sqlite>, PathBuf) {
        let database_path = std::env::temp_dir().join(format!(
            "distributed-projection-resume-{}.sqlite",
            uuid::Uuid::now_v7()
        ));
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(&database_path)
                    .create_if_missing(true)
                    .journal_mode(SqliteJournalMode::Wal),
            )
            .await
            .unwrap();
        let repository = SqlxRepository::<sqlx::Sqlite>::new(pool)
            .with_projection_change_retention(
                ProjectionChangeRetention::new(max_retained_changes).unwrap(),
            );
        repository.migrate().await.unwrap();
        let mut registry = TableSchemaRegistry::new();
        registry.register_schema(schema().clone()).unwrap();
        repository
            .bootstrap_table_schema_for_dev(&registry)
            .await
            .unwrap();
        repository
            .register_projection_models(&topology(), &[ownership()])
            .await
            .unwrap();
        (repository, database_path)
    }

    async fn remove_wal_database(repository: SqlxRepository<sqlx::Sqlite>, path: &Path) {
        repository.pool().close().await;
        for candidate in [
            path.to_path_buf(),
            path.with_extension("sqlite-wal"),
            path.with_extension("sqlite-shm"),
        ] {
            match std::fs::remove_file(&candidate) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => panic!(
                    "remove SQLite projection resume test file {}: {error}",
                    candidate.display()
                ),
            }
        }
    }

    async fn unregistered_repository() -> SqlxRepository<sqlx::Sqlite> {
        let repository = SqlxRepository::<sqlx::Sqlite>::connect_and_migrate("sqlite::memory:")
            .await
            .unwrap();
        let mut registry = TableSchemaRegistry::new();
        registry.register_schema(schema().clone()).unwrap();
        repository
            .bootstrap_table_schema_for_dev(&registry)
            .await
            .unwrap();
        repository
    }

    fn upsert_table_mutation(id: &str) -> TableMutation {
        let key = RowKey::new([("id", RowValue::String(id.into()))]);
        let mut values = RowValues::new();
        values.insert("id", RowValue::String(id.into()));
        TableMutation::UpsertRow(TableRowMutation {
            schema: schema(),
            key,
            values,
            expected_version: ExpectedVersion::Any,
            mode: RowWriteMode::Upsert,
        })
    }

    fn valued_mutation(
        value: u64,
        expectation: ProjectionRecordExpectation,
    ) -> ProjectionRecordMutation {
        let mut values = RowValues::new();
        values.insert("id", RowValue::String("todo-1".into()));
        values.insert("value", RowValue::String(value.to_string()));
        ProjectionRecordMutation::new(
            record_scope(),
            TableMutation::UpsertRow(TableRowMutation {
                schema: schema(),
                key: record_key(),
                values,
                expected_version: ExpectedVersion::Any,
                mode: RowWriteMode::Upsert,
            }),
            expectation,
            ProjectionMutationKind::Upsert,
        )
        .unwrap()
    }

    fn snapshot_request() -> ProjectionQuerySnapshotRequest {
        ProjectionQuerySnapshotRequest::new(
            &scope_codec(),
            Some(&serde_json::json!("tenant-sql")),
            "SqlTodoView",
            record_key(),
            vec![ProjectionCheckpointProbe::new(
                topology(),
                partition(),
                source("todo_stream", b"todo-1"),
                ProjectionEpoch::new("source-v1").unwrap(),
                ProjectionGeneration::initial(),
            )],
        )
        .unwrap()
    }

    fn mutation(
        expectation: ProjectionRecordExpectation,
        kind: ProjectionMutationKind,
    ) -> ProjectionRecordMutation {
        let table = match kind {
            ProjectionMutationKind::Delete => TableMutation::DeleteRow(DeleteTableRowMutation {
                schema: schema(),
                key: RowKey::new([("id", RowValue::String("todo-1".into()))]),
                expected_version: ExpectedVersion::Any,
            }),
            ProjectionMutationKind::Upsert | ProjectionMutationKind::Recreate => {
                upsert_table_mutation("todo-1")
            }
        };
        ProjectionRecordMutation::new(record_scope(), table, expectation, kind).unwrap()
    }

    fn batch(
        trusted: TrustedProjectionInput,
        mutations: Vec<ProjectionRecordMutation>,
        observations: Vec<ProjectionObservationRequest>,
    ) -> ProjectionCommitBatch {
        ProjectionCommitBatch {
            input: trusted,
            change_epoch: change_epoch(),
            ownership: vec![ownership()],
            mutations,
            observations,
        }
    }

    async fn row_exists(repository: &SqlxRepository<sqlx::Sqlite>) -> bool {
        let row = sqlx::query("SELECT 1 AS present FROM sql_todo_views WHERE id = ? LIMIT 1")
            .bind("todo-1")
            .fetch_optional(repository.pool())
            .await
            .unwrap();
        row.and_then(|row| row.try_get::<i64, _>("present").ok())
            .is_some()
    }

    fn assert_query_snapshot_is_coherent(snapshot: &ProjectionQuerySnapshot) {
        let row = snapshot.row.as_ref().expect("physical row");
        let record = snapshot.record.as_ref().expect("record metadata");
        let checkpoint = snapshot.checkpoints[0]
            .checkpoint
            .as_ref()
            .expect("source checkpoint");
        let value = match row.get("value") {
            Some(RowValue::String(value)) => value.parse::<u64>().unwrap(),
            other => panic!("unexpected query snapshot value {other:?}"),
        };
        assert_eq!(value, record.revision.revision());
        assert_eq!(value, checkpoint.input().position());
        assert_eq!(record.change, *checkpoint.change());
        assert_eq!(
            snapshot.change_head.as_ref(),
            Some(checkpoint.change()),
            "live resume head must come from the same SQL statement snapshot"
        );
        assert_eq!(snapshot.compacted_through, 0);
    }

    #[tokio::test]
    async fn sqlite_query_snapshot_never_mixes_row_revision_checkpoint_or_resume_head() {
        let repository = repository().await;
        let first = repository
            .commit_projection(batch(
                input(
                    1,
                    b"snapshot-1",
                    "snapshot-message-1",
                    "snapshot-cause-1",
                    ProjectionGeneration::initial(),
                ),
                vec![valued_mutation(1, ProjectionRecordExpectation::Missing)],
                Vec::new(),
            ))
            .await
            .unwrap();
        let mut expected = first.records[0].revision.clone();
        assert_query_snapshot_is_coherent(
            &repository
                .projection_query_snapshot(&snapshot_request())
                .await
                .unwrap(),
        );

        let writer_repository = repository.clone();
        let writer = tokio::spawn(async move {
            for position in 2..=32 {
                let committed = writer_repository
                    .commit_projection(batch(
                        input(
                            position,
                            format!("snapshot-{position}").as_bytes(),
                            &format!("snapshot-message-{position}"),
                            &format!("snapshot-cause-{position}"),
                            ProjectionGeneration::initial(),
                        ),
                        vec![valued_mutation(
                            position,
                            ProjectionRecordExpectation::Exact(expected),
                        )],
                        Vec::new(),
                    ))
                    .await
                    .unwrap();
                expected = committed.records[0].revision.clone();
                tokio::task::yield_now().await;
            }
        });

        while !writer.is_finished() {
            let batch = repository
                .projection_query_snapshot_batch(
                    &ProjectionQuerySnapshotBatchRequest::new(vec![
                        snapshot_request(),
                        snapshot_request(),
                    ])
                    .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(batch.snapshots[0], batch.snapshots[1]);
            assert_query_snapshot_is_coherent(&batch.snapshots[0]);
            tokio::task::yield_now().await;
        }
        writer.await.unwrap();
        assert_query_snapshot_is_coherent(
            &repository
                .projection_query_snapshot(&snapshot_request())
                .await
                .unwrap(),
        );
    }

    #[tokio::test]
    async fn sqlite_input_disposition_is_read_only_exact_and_repair_fenced() {
        let repository = repository().await;
        let first_input = input(
            1,
            b"preflight-one",
            "preflight-message-1",
            "preflight-cause-1",
            ProjectionGeneration::initial(),
        );
        assert_eq!(
            repository
                .projection_input_disposition(&first_input)
                .await
                .unwrap(),
            ProjectionInputDisposition::Pending
        );
        assert_eq!(
            repository
                .projection_partition_runtime_state(&topology(), &partition())
                .await
                .unwrap(),
            None,
            "a preflight read must not create a projection partition"
        );
        let capability_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM projection_source_capabilities")
                .fetch_one(repository.pool())
                .await
                .unwrap();
        assert_eq!(
            capability_count, 0,
            "a preflight read must not register a source capability"
        );

        let applied = repository
            .commit_projection(batch(first_input.clone(), Vec::new(), Vec::new()))
            .await
            .unwrap();
        assert_eq!(
            repository
                .projection_input_disposition(&first_input)
                .await
                .unwrap(),
            ProjectionInputDisposition::Duplicate(applied.checkpoint.unwrap())
        );
        let stale = input(
            0,
            b"preflight-stale",
            "preflight-message-0",
            "preflight-cause-0",
            ProjectionGeneration::initial(),
        );
        assert!(matches!(
            repository
                .projection_input_disposition(&stale)
                .await
                .unwrap(),
            ProjectionInputDisposition::Stale(checkpoint)
                if checkpoint.input().position() == 1
        ));
        let corrupted = input(
            1,
            b"preflight-corrupt",
            "preflight-message-1",
            "preflight-cause-1",
            ProjectionGeneration::initial(),
        );
        assert!(matches!(
            repository.projection_input_disposition(&corrupted).await,
            Err(ProjectionProtocolError::InputCorruption)
        ));
        let reused_message = input(
            2,
            b"preflight-two",
            "preflight-message-1",
            "preflight-cause-2",
            ProjectionGeneration::initial(),
        );
        assert!(matches!(
            repository
                .projection_input_disposition(&reused_message)
                .await,
            Err(ProjectionProtocolError::MessageIdReuse { message_id })
                if message_id == "preflight-message-1"
        ));

        let failed_input = input(
            2,
            b"preflight-two",
            "preflight-message-2",
            "preflight-cause-2",
            ProjectionGeneration::initial(),
        );
        repository
            .record_projection_failure(
                ProjectionFailureBatch::new(
                    failed_input.clone(),
                    change_epoch(),
                    "preflight-failure-2",
                    "decode_error",
                    b"bad payload".to_vec(),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        assert!(matches!(
            repository
                .projection_input_disposition(&failed_input)
                .await,
            Err(ProjectionProtocolError::PartitionStopped { failure_id })
                if failure_id == "preflight-failure-2"
        ));

        let generation = repository
            .repair_projection(&topology(), &partition(), "preflight-failure-2")
            .await
            .unwrap();
        let retry = input(
            2,
            b"preflight-two",
            "preflight-message-2",
            "preflight-cause-2",
            generation,
        );
        assert_eq!(
            repository
                .projection_input_disposition(&retry)
                .await
                .unwrap(),
            ProjectionInputDisposition::Pending
        );
        assert!(matches!(
            repository.projection_input_disposition(&first_input).await,
            Err(ProjectionProtocolError::GenerationFenced {
                expected: 2,
                actual: 1
            })
        ));
        assert!(matches!(
            repository
                .projection_input_disposition(&input(
                    3,
                    b"preflight-later",
                    "preflight-message-3",
                    "preflight-cause-3",
                    generation,
                ))
                .await,
            Err(ProjectionProtocolError::IncomparableInput)
        ));

        let repaired = repository
            .commit_projection(batch(retry.clone(), Vec::new(), Vec::new()))
            .await
            .unwrap();
        assert_eq!(
            repository
                .projection_input_disposition(&retry)
                .await
                .unwrap(),
            ProjectionInputDisposition::Duplicate(repaired.checkpoint.unwrap())
        );
    }

    #[tokio::test]
    async fn sqlite_obligation_and_unpartitioned_live_evidence_are_exact_and_durable() {
        let evidence_repository = repository().await;
        let scope = record_scope();
        let live_request = ProjectionLiveRecordBatchRequest::new(vec![
            crate::projection_protocol::ProjectionLiveRecordRequest::new(
                &scope_codec(),
                "SqlTodoView",
                record_key(),
            )
            .unwrap(),
        ])
        .unwrap();
        assert_eq!(
            evidence_repository
                .projection_live_record_batch(&live_request)
                .await
                .unwrap()
                .records,
            vec![None]
        );

        let created = evidence_repository
            .commit_projection(batch(
                input(
                    1,
                    b"evidence-created",
                    "evidence-message-1",
                    "evidence-cause",
                    ProjectionGeneration::initial(),
                ),
                vec![mutation(
                    ProjectionRecordExpectation::Missing,
                    ProjectionMutationKind::Upsert,
                )],
                vec![ProjectionObservationRequest {
                    kind: ProjectionObservationKind::Record,
                    target: ProjectionObservationTarget::StagedRecord(scope.clone()),
                }],
            ))
            .await
            .unwrap();
        assert_eq!(
            evidence_repository
                .projection_live_record_batch(&live_request)
                .await
                .unwrap()
                .records[0]
                .as_ref()
                .unwrap()
                .revision
                .scope(),
            &scope
        );
        let observed = crate::projection_protocol::ProjectionObligationEvidenceRequest::new(
            "evidence-cause",
            scope.clone(),
            ProjectionObservationKind::Record,
        )
        .unwrap();
        let pending = crate::projection_protocol::ProjectionObligationEvidenceRequest::new(
            "pending-cause",
            scope.clone(),
            ProjectionObservationKind::Record,
        )
        .unwrap();
        let before_failure = evidence_repository
            .projection_obligation_evidence_batch(
                &ProjectionObligationEvidenceBatchRequest::new(vec![
                    observed.clone(),
                    pending.clone(),
                ])
                .unwrap(),
            )
            .await
            .unwrap();
        assert!(matches!(
            &before_failure.evidence[0],
            ProjectionObligationEvidence::Observed(observation)
                if observation.change == created.changes[0].cursor
        ));
        assert_eq!(
            before_failure.evidence[1],
            ProjectionObligationEvidence::Pending
        );

        let failure = evidence_repository
            .record_projection_failure(
                ProjectionFailureBatch::new(
                    input(
                        2,
                        b"evidence-failed",
                        "evidence-message-2",
                        "evidence-cause",
                        ProjectionGeneration::initial(),
                    ),
                    change_epoch(),
                    "evidence-failure",
                    "decode_error",
                    b"bad evidence payload".to_vec(),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        evidence_repository
            .compact_projection_changes(&failure.change)
            .await
            .unwrap();
        let after_failure = evidence_repository
            .projection_obligation_evidence_batch(
                &ProjectionObligationEvidenceBatchRequest::new(vec![observed, pending]).unwrap(),
            )
            .await
            .unwrap();
        assert!(matches!(
            &after_failure.evidence[0],
            ProjectionObligationEvidence::TerminalFailure(stored)
                if stored == &failure
        ));
        assert_eq!(
            after_failure.evidence[1],
            ProjectionObligationEvidence::Pending
        );

        let moved_repository = repository().await;
        let old_scope = record_scope();
        let old = moved_repository
            .commit_projection(batch(
                input(
                    1,
                    b"move-old",
                    "move-message-1",
                    "move-cause-1",
                    ProjectionGeneration::initial(),
                ),
                vec![mutation(
                    ProjectionRecordExpectation::Missing,
                    ProjectionMutationKind::Upsert,
                )],
                Vec::new(),
            ))
            .await
            .unwrap()
            .records
            .remove(0);
        moved_repository
            .commit_projection(batch(
                input(
                    2,
                    b"move-delete",
                    "move-message-2",
                    "move-cause-2",
                    ProjectionGeneration::initial(),
                ),
                vec![mutation(
                    ProjectionRecordExpectation::Exact(old.revision),
                    ProjectionMutationKind::Delete,
                )],
                Vec::new(),
            ))
            .await
            .unwrap();
        assert_eq!(
            moved_repository
                .projection_live_record_batch(&live_request)
                .await
                .unwrap()
                .records,
            vec![None]
        );

        let new_partition = scope_codec()
            .encode_partition(Some(&serde_json::json!("tenant-moved")))
            .unwrap();
        let new_scope = scope_codec()
            .encode_row_scope_in_partition("SqlTodoView", new_partition.clone(), &record_key())
            .unwrap();
        let new_input = TrustedProjectionInput::mint(
            ProjectionInputCursor::new(
                topology(),
                new_partition,
                source("todo_stream", b"todo-1"),
                ProjectionEpoch::new("source-v1").unwrap(),
                1,
            )
            .unwrap(),
            ProjectionInputFingerprint::from_canonical_bytes(b"move-new"),
            "move-message-3",
            "move-cause-3",
            ProjectionGeneration::initial(),
            true,
        )
        .unwrap();
        moved_repository
            .commit_projection(ProjectionCommitBatch {
                input: new_input,
                change_epoch: change_epoch(),
                ownership: vec![ownership()],
                mutations: vec![ProjectionRecordMutation::new(
                    new_scope.clone(),
                    upsert_table_mutation("todo-1"),
                    ProjectionRecordExpectation::Missing,
                    ProjectionMutationKind::Upsert,
                )
                .unwrap()],
                observations: Vec::new(),
            })
            .await
            .unwrap();
        assert_eq!(
            moved_repository
                .projection_live_record_batch(&live_request)
                .await
                .unwrap()
                .records[0]
                .as_ref()
                .unwrap()
                .revision
                .scope(),
            &new_scope
        );

        let old_partition_hash = partition().digest();
        assert!(
            sqlx::query(
                "UPDATE projection_records SET tombstone = 0 \
                 WHERE topology_hash = ? AND partition_hash = ? AND model_name = ?",
            )
            .bind(topology().digest().as_slice())
            .bind(old_partition_hash.as_slice())
            .bind("SqlTodoView")
            .execute(moved_repository.pool())
            .await
            .is_err(),
            "the partial unique index must reject a second live partition"
        );
        sqlx::query("DROP INDEX projection_records_unique_live_identity")
            .execute(moved_repository.pool())
            .await
            .unwrap();
        sqlx::query(
            "UPDATE projection_records SET tombstone = 0 \
             WHERE topology_hash = ? AND partition_hash = ? AND model_name = ?",
        )
        .bind(topology().digest().as_slice())
        .bind(old_partition_hash.as_slice())
        .bind("SqlTodoView")
        .execute(moved_repository.pool())
        .await
        .unwrap();
        assert!(matches!(
            moved_repository
                .projection_live_record_batch(&live_request)
                .await,
            Err(ProjectionProtocolError::InvalidBatch(message))
                if message.contains("ambiguous")
        ));

        let drift_repository = repository().await;
        drift_repository
            .commit_projection(batch(
                input(
                    1,
                    b"drift",
                    "drift-message-1",
                    "drift-cause-1",
                    ProjectionGeneration::initial(),
                ),
                vec![mutation(
                    ProjectionRecordExpectation::Missing,
                    ProjectionMutationKind::Upsert,
                )],
                Vec::new(),
            ))
            .await
            .unwrap();
        sqlx::query(
            "UPDATE projection_records SET canonical_key_bytes = ? \
             WHERE topology_hash = ? AND model_name = ?",
        )
        .bind(b"corrupt-key".as_slice())
        .bind(topology().digest().as_slice())
        .bind("SqlTodoView")
        .execute(drift_repository.pool())
        .await
        .unwrap();
        assert!(matches!(
            drift_repository
                .projection_live_record_batch(&live_request)
                .await,
            Err(ProjectionProtocolError::InvalidBatch(message))
                if message.contains("canonical key")
        ));

        // Keep the compiler from treating the old exact scope as an incidental
        // local: it is the durable tombstone retained across the move.
        assert_ne!(old_scope, new_scope);
    }

    #[tokio::test]
    async fn sqlite_receipts_source_fences_and_raw_write_fence_are_exact() {
        let repository = repository().await;
        let raw_error = repository
            .commit_write_plan(TableWritePlan::new(vec![upsert_table_mutation("todo-1")]))
            .await
            .unwrap_err();
        assert!(matches!(
            raw_error,
            TableStoreError::CausalWriteRequired { ref table } if table == "sql_todo_views"
        ));

        let applied = repository
            .commit_projection(batch(
                input(
                    1,
                    b"one",
                    "message-1",
                    "cause-1",
                    ProjectionGeneration::initial(),
                ),
                vec![mutation(
                    ProjectionRecordExpectation::Missing,
                    ProjectionMutationKind::Upsert,
                )],
                Vec::new(),
            ))
            .await
            .unwrap();
        assert_eq!(applied.outcome, ProjectionCommitOutcome::Applied);
        assert!(row_exists(&repository).await);

        let duplicate = repository
            .commit_projection(batch(
                input(
                    1,
                    b"one",
                    "message-1",
                    "cause-1",
                    ProjectionGeneration::initial(),
                ),
                Vec::new(),
                Vec::new(),
            ))
            .await
            .unwrap();
        assert_eq!(duplicate.outcome, ProjectionCommitOutcome::Duplicate);
        assert_eq!(
            duplicate.checkpoint.as_ref().unwrap().change(),
            applied.checkpoint.as_ref().unwrap().change()
        );

        assert!(matches!(
            repository
                .commit_projection(batch(
                    input(
                        1,
                        b"changed",
                        "new-message",
                        "cause-1",
                        ProjectionGeneration::initial(),
                    ),
                    Vec::new(),
                    Vec::new(),
                ))
                .await,
            Err(ProjectionProtocolError::InputCorruption)
        ));

        repository
            .commit_projection(batch(
                input(
                    2,
                    b"two",
                    "message-2",
                    "cause-2",
                    ProjectionGeneration::initial(),
                ),
                Vec::new(),
                Vec::new(),
            ))
            .await
            .unwrap();
        let old_after_advance = repository
            .commit_projection(batch(
                input(
                    1,
                    b"one",
                    "message-1",
                    "cause-1",
                    ProjectionGeneration::initial(),
                ),
                Vec::new(),
                Vec::new(),
            ))
            .await
            .unwrap();
        assert_eq!(
            old_after_advance.outcome,
            ProjectionCommitOutcome::Duplicate
        );
        assert!(matches!(
            repository
                .commit_projection(batch(
                    input(
                        1,
                        b"changed-after-advance",
                        "new-message-after-advance",
                        "cause-1",
                        ProjectionGeneration::initial(),
                    ),
                    Vec::new(),
                    Vec::new(),
                ))
                .await,
            Err(ProjectionProtocolError::InputCorruption)
        ));
        assert!(matches!(
            repository
                .commit_projection(batch(
                    input(
                        3,
                        b"three",
                        "message-1",
                        "cause-3",
                        ProjectionGeneration::initial(),
                    ),
                    Vec::new(),
                    Vec::new(),
                ))
                .await,
            Err(ProjectionProtocolError::MessageIdReuse { .. })
        ));
        let stale = repository
            .commit_projection(batch(
                input(
                    0,
                    b"stale",
                    "stale-message",
                    "stale-cause",
                    ProjectionGeneration::initial(),
                ),
                Vec::new(),
                Vec::new(),
            ))
            .await
            .unwrap();
        assert_eq!(stale.outcome, ProjectionCommitOutcome::StaleInput);
        let changed_capability = TrustedProjectionInput::mint(
            input_cursor(3),
            ProjectionInputFingerprint::from_canonical_bytes(b"changed-capability"),
            "capability-message",
            "capability-cause",
            ProjectionGeneration::initial(),
            false,
        )
        .unwrap();
        assert!(matches!(
            repository
                .commit_projection(batch(changed_capability, Vec::new(), Vec::new()))
                .await,
            Err(ProjectionProtocolError::InputCorruption)
        ));
        assert!(matches!(
            repository
                .commit_projection(batch(
                    input(
                        4,
                        b"gap",
                        "message-4",
                        "cause-4",
                        ProjectionGeneration::initial(),
                    ),
                    Vec::new(),
                    Vec::new(),
                ))
                .await,
            Err(ProjectionProtocolError::IncomparableInput)
        ));

        let other_source = source("audit_stream", b"audit-1");
        let other = input_for_source(
            other_source.clone(),
            41,
            b"audit",
            "audit-message",
            "audit-cause",
        );
        repository
            .commit_projection(batch(other, Vec::new(), Vec::new()))
            .await
            .unwrap();
        assert_eq!(
            repository
                .projection_checkpoint(
                    &input_cursor_for(other_source, 0, "source-v1"),
                    ProjectionGeneration::initial(),
                )
                .await
                .unwrap()
                .unwrap()
                .input()
                .position(),
            41
        );

        let mut transactional = CommitBatch::empty();
        transactional
            .read_model_plans
            .push(TableWritePlan::new(vec![upsert_table_mutation("todo-1")]));
        assert!(matches!(
            repository.commit_batch(transactional).await,
            Err(crate::RepositoryError::CausalWriteRequired { .. })
        ));
    }

    #[tokio::test]
    async fn sqlite_message_identity_is_topology_wide_across_projection_partitions() {
        let repository = repository().await;
        repository
            .commit_projection(batch(
                input(
                    1,
                    b"topology-wide-message",
                    "topology-wide-message",
                    "topology-wide-cause",
                    ProjectionGeneration::initial(),
                ),
                Vec::new(),
                Vec::new(),
            ))
            .await
            .unwrap();

        let other_partition = ProjectionScopeCodec::new(topology())
            .encode_partition(Some(&serde_json::json!("tenant-b")))
            .unwrap();
        let remapped = TrustedProjectionInput::mint(
            ProjectionInputCursor::new(
                topology(),
                other_partition,
                source("todo_stream", b"todo-1"),
                ProjectionEpoch::new("source-v1").unwrap(),
                1,
            )
            .unwrap(),
            ProjectionInputFingerprint::from_canonical_bytes(b"topology-wide-message"),
            "topology-wide-message",
            "topology-wide-cause",
            ProjectionGeneration::initial(),
            true,
        )
        .unwrap();

        assert!(matches!(
            repository
                .commit_projection(batch(remapped, Vec::new(), Vec::new()))
                .await,
            Err(ProjectionProtocolError::MessageIdReuse { message_id })
                if message_id == "topology-wide-message"
        ));
    }

    #[tokio::test]
    async fn sqlite_row_failure_rolls_back_protocol_receipt_inbox_and_domain_row() {
        let repository = repository().await;
        sqlx::query(
            "CREATE TRIGGER fail_sql_todo_insert BEFORE INSERT ON sql_todo_views \
             BEGIN SELECT RAISE(ABORT, 'forced projection row failure'); END",
        )
        .execute(repository.pool())
        .await
        .unwrap();
        assert!(matches!(
            repository
                .commit_projection(batch(
                    input(
                        1,
                        b"rollback",
                        "rollback-message",
                        "rollback-cause",
                        ProjectionGeneration::initial(),
                    ),
                    vec![mutation(
                        ProjectionRecordExpectation::Missing,
                        ProjectionMutationKind::Upsert,
                    )],
                    Vec::new(),
                ))
                .await,
            Err(ProjectionProtocolError::Table(_))
        ));
        assert!(!row_exists(&repository).await);
        assert!(repository
            .projection_record(&record_scope())
            .await
            .unwrap()
            .is_none());
        assert!(repository
            .projection_checkpoint(&input_cursor(1), ProjectionGeneration::initial(),)
            .await
            .unwrap()
            .is_none());
        sqlx::query("DROP TRIGGER fail_sql_todo_insert")
            .execute(repository.pool())
            .await
            .unwrap();
        assert_eq!(
            repository
                .commit_projection(batch(
                    input(
                        1,
                        b"rollback",
                        "rollback-message",
                        "rollback-cause",
                        ProjectionGeneration::initial(),
                    ),
                    vec![mutation(
                        ProjectionRecordExpectation::Missing,
                        ProjectionMutationKind::Upsert,
                    )],
                    Vec::new(),
                ))
                .await
                .unwrap()
                .outcome,
            ProjectionCommitOutcome::Applied
        );
    }

    #[tokio::test]
    async fn sqlite_rejects_tampered_failure_digest_before_any_protocol_write() {
        let repository = repository().await;
        let mut failure = ProjectionFailureBatch::new(
            input(
                1,
                b"tampered-failure",
                "tampered-failure-message",
                "tampered-failure-cause",
                ProjectionGeneration::initial(),
            ),
            change_epoch(),
            "tampered-failure-id",
            "decode_error",
            b"shape-valid failure details".to_vec(),
        )
        .unwrap();
        failure.failure_digest[0] ^= 0xff;

        assert!(matches!(
            repository.record_projection_failure(failure).await,
            Err(ProjectionProtocolError::InvalidBatch(message))
                if message == "projection failure digest does not match its exact bytes"
        ));
        for (table, count_sql) in [
            (
                "projection_partitions",
                "SELECT COUNT(*) FROM projection_partitions",
            ),
            (
                "projection_changes",
                "SELECT COUNT(*) FROM projection_changes",
            ),
            (
                "projection_failures",
                "SELECT COUNT(*) FROM projection_failures",
            ),
            (
                "projection_input_identities",
                "SELECT COUNT(*) FROM projection_input_identities",
            ),
            (
                "projection_input_receipts",
                "SELECT COUNT(*) FROM projection_input_receipts",
            ),
            ("consumer_inbox", "SELECT COUNT(*) FROM consumer_inbox"),
        ] {
            let count: i64 = sqlx::query_scalar(count_sql)
                .fetch_one(repository.pool())
                .await
                .unwrap();
            assert_eq!(
                count, 0,
                "tampered failure validation must precede writes to {table}"
            );
        }
    }

    #[tokio::test]
    async fn sqlite_tombstones_observations_failure_repair_and_compaction_conform() {
        let repository = repository().await;
        let scope = record_scope();
        assert_eq!(
            repository
                .projection_partition_runtime_state(&topology(), &partition())
                .await
                .unwrap(),
            None
        );
        let created = repository
            .commit_projection(batch(
                input(
                    1,
                    b"create",
                    "message-1",
                    "stable-cause",
                    ProjectionGeneration::initial(),
                ),
                vec![mutation(
                    ProjectionRecordExpectation::Missing,
                    ProjectionMutationKind::Upsert,
                )],
                vec![ProjectionObservationRequest {
                    kind: ProjectionObservationKind::Record,
                    target: ProjectionObservationTarget::StagedRecord(scope.clone()),
                }],
            ))
            .await
            .unwrap();
        assert_eq!(created.changes.len(), 1);
        let earliest = repository
            .projection_observation("stable-cause", &scope, ProjectionObservationKind::Record)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(earliest.change, created.changes[0].cursor);

        let deleted = repository
            .commit_projection(batch(
                input(
                    2,
                    b"delete",
                    "message-2",
                    "stable-cause",
                    ProjectionGeneration::initial(),
                ),
                vec![mutation(
                    ProjectionRecordExpectation::Exact(created.records[0].revision.clone()),
                    ProjectionMutationKind::Delete,
                )],
                vec![ProjectionObservationRequest {
                    kind: ProjectionObservationKind::Record,
                    target: ProjectionObservationTarget::StagedRecord(scope.clone()),
                }],
            ))
            .await
            .unwrap();
        assert!(deleted.records[0].tombstone);
        assert!(!row_exists(&repository).await);
        assert_eq!(
            repository
                .projection_observation("stable-cause", &scope, ProjectionObservationKind::Record,)
                .await
                .unwrap()
                .unwrap(),
            earliest
        );
        assert!(matches!(
            repository
                .commit_projection(batch(
                    input(
                        3,
                        b"plain-upsert",
                        "message-3",
                        "cause-3",
                        ProjectionGeneration::initial(),
                    ),
                    vec![mutation(
                        ProjectionRecordExpectation::Exact(deleted.records[0].revision.clone(),),
                        ProjectionMutationKind::Upsert,
                    )],
                    Vec::new(),
                ))
                .await,
            Err(ProjectionProtocolError::RecordTombstoned { .. })
        ));
        let recreated = repository
            .commit_projection(batch(
                input(
                    3,
                    b"recreate",
                    "message-3b",
                    "cause-3",
                    ProjectionGeneration::initial(),
                ),
                vec![mutation(
                    ProjectionRecordExpectation::Exact(deleted.records[0].revision.clone()),
                    ProjectionMutationKind::Recreate,
                )],
                vec![ProjectionObservationRequest {
                    kind: ProjectionObservationKind::Dependency,
                    target: ProjectionObservationTarget::Dependency(scope.clone()),
                }],
            ))
            .await
            .unwrap();
        assert_eq!(recreated.records[0].revision.incarnation(), 2);
        assert_eq!(recreated.records[0].revision.revision(), 1);
        assert_eq!(recreated.changes.len(), 2);
        assert!(repository
            .projection_observation("cause-3", &scope, ProjectionObservationKind::Dependency,)
            .await
            .unwrap()
            .unwrap()
            .revision
            .is_none());

        let failure_batch = ProjectionFailureBatch::new(
            input(
                4,
                b"failure",
                "message-4",
                "cause-4",
                ProjectionGeneration::initial(),
            ),
            change_epoch(),
            "failure-4",
            "decode_error",
            b"bad payload".to_vec(),
        )
        .unwrap();
        let failure = repository
            .record_projection_failure(failure_batch.clone())
            .await
            .unwrap();
        assert_eq!(
            repository
                .record_projection_failure(failure_batch.clone())
                .await
                .unwrap(),
            failure
        );
        assert_eq!(
            repository
                .projection_failure(&topology(), &partition(), "failure-4")
                .await
                .unwrap(),
            Some(failure.clone())
        );
        assert!(failure.gap_free);
        let stopped = repository
            .projection_partition_runtime_state(&topology(), &partition())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stopped.active_generation, ProjectionGeneration::initial());
        assert_eq!(stopped.stopped_failure_id.as_deref(), Some("failure-4"));
        assert_eq!(stopped.pending_retry, None);
        assert!(matches!(
            repository
                .commit_projection(batch(
                    input(
                        5,
                        b"blocked",
                        "message-5",
                        "cause-5",
                        ProjectionGeneration::initial(),
                    ),
                    Vec::new(),
                    Vec::new(),
                ))
                .await,
            Err(ProjectionProtocolError::PartitionStopped { .. })
        ));
        assert!(matches!(
            repository
                .commit_projection(batch(
                    input(
                        4,
                        b"changed-while-stopped",
                        "changed-message-4",
                        "cause-4",
                        ProjectionGeneration::initial(),
                    ),
                    Vec::new(),
                    Vec::new(),
                ))
                .await,
            Err(ProjectionProtocolError::InputCorruption)
        ));
        assert!(matches!(
            repository
                .commit_projection(batch(
                    input(
                        5,
                        b"reused-message-while-stopped",
                        "message-4",
                        "cause-5",
                        ProjectionGeneration::initial(),
                    ),
                    Vec::new(),
                    Vec::new(),
                ))
                .await,
            Err(ProjectionProtocolError::MessageIdReuse { .. })
        ));
        assert!(matches!(
            repository
                .commit_projection(batch(
                    input(
                        4,
                        b"failure",
                        "message-4",
                        "cause-4",
                        ProjectionGeneration::initial(),
                    ),
                    Vec::new(),
                    Vec::new(),
                ))
                .await,
            Err(ProjectionProtocolError::PartitionStopped { .. })
        ));
        let generation = repository
            .repair_projection(&topology(), &partition(), "failure-4")
            .await
            .unwrap();
        assert_eq!(generation.get(), 2);
        let repaired = repository
            .projection_partition_runtime_state(&topology(), &partition())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(repaired.active_generation, generation);
        assert_eq!(repaired.stopped_failure_id, None);
        let retry = repaired.pending_retry.unwrap();
        assert_eq!(retry.failure_id, "failure-4");
        assert_eq!(retry.input, failure.input);
        assert_eq!(retry.input_fingerprint, failure.input_fingerprint);
        assert_eq!(retry.message_id, failure.message_id);
        assert_eq!(retry.causation_id, failure.causation_id);
        assert_eq!(retry.failed_generation, failure.generation);
        assert_eq!(retry.gap_free, failure.gap_free);
        assert!(matches!(
            repository.record_projection_failure(failure_batch).await,
            Err(ProjectionProtocolError::GenerationFenced {
                expected: 2,
                actual: 1
            })
        ));
        assert!(matches!(
            repository
                .commit_projection(batch(
                    input(
                        4,
                        b"changed-old-generation",
                        "changed-old-message-4",
                        "cause-4",
                        ProjectionGeneration::initial(),
                    ),
                    Vec::new(),
                    Vec::new(),
                ))
                .await,
            Err(ProjectionProtocolError::InputCorruption)
        ));
        assert!(matches!(
            repository
                .commit_projection(batch(
                    input(
                        6,
                        b"unknown-old-generation",
                        "unknown-old-message-6",
                        "unknown-old-cause-6",
                        ProjectionGeneration::initial(),
                    ),
                    Vec::new(),
                    Vec::new(),
                ))
                .await,
            Err(ProjectionProtocolError::GenerationFenced {
                expected: 2,
                actual: 1
            })
        ));
        let changed_old_capability = TrustedProjectionInput::mint(
            input_cursor(6),
            ProjectionInputFingerprint::from_canonical_bytes(b"changed-old-capability"),
            "changed-old-capability-message",
            "changed-old-capability-cause",
            ProjectionGeneration::initial(),
            false,
        )
        .unwrap();
        assert!(matches!(
            repository
                .commit_projection(batch(changed_old_capability, Vec::new(), Vec::new()))
                .await,
            Err(ProjectionProtocolError::InputCorruption)
        ));
        assert_eq!(
            repository
                .commit_projection(batch(
                    input(3, b"recreate", "message-3b", "cause-3", generation),
                    Vec::new(),
                    Vec::new(),
                ))
                .await
                .unwrap()
                .outcome,
            ProjectionCommitOutcome::Duplicate
        );
        assert_eq!(
            repository
                .commit_projection(batch(
                    input(2, b"delete", "message-2", "stable-cause", generation),
                    Vec::new(),
                    Vec::new(),
                ))
                .await
                .unwrap()
                .outcome,
            ProjectionCommitOutcome::StaleInput
        );
        assert!(matches!(
            repository
                .commit_projection(batch(
                    input(5, b"later", "message-5", "cause-5", generation),
                    Vec::new(),
                    Vec::new(),
                ))
                .await,
            Err(ProjectionProtocolError::IncomparableInput)
        ));
        assert!(matches!(
            repository
                .commit_projection(batch(
                    input(4, b"retry", "message-4b", "cause-4", generation),
                    Vec::new(),
                    Vec::new(),
                ))
                .await,
            Err(ProjectionProtocolError::InputCorruption)
        ));
        let repaired = repository
            .commit_projection(batch(
                input(4, b"failure", "message-4", "cause-4", generation),
                Vec::new(),
                Vec::new(),
            ))
            .await
            .unwrap();
        assert_eq!(repaired.changes[0].kind, ProjectionChangeKind::Checkpoint);
        assert_eq!(
            repository
                .commit_projection(batch(
                    input(5, b"later", "message-5", "cause-5", generation),
                    Vec::new(),
                    Vec::new(),
                ))
                .await
                .unwrap()
                .outcome,
            ProjectionCommitOutcome::Applied
        );

        let compacted = repository
            .compact_projection_changes(&created.changes[0].cursor)
            .await
            .unwrap();
        assert_eq!(compacted, created.changes[0].cursor.position());
        assert!(matches!(
            repository
                .projection_changes(
                    &topology(),
                    &partition(),
                    Some(&created.changes[0].cursor),
                    100,
                )
                .await
                .unwrap(),
            ProjectionChangeRead::Changes {
                compacted_through,
                ref changes,
                ..
            } if compacted_through == compacted && !changes.is_empty()
        ));
        assert!(matches!(
            repository
                .projection_changes(&topology(), &partition(), None, 100)
                .await
                .unwrap(),
            ProjectionChangeRead::ResetRequired {
                compacted_through,
                ..
            } if compacted_through == compacted
        ));
        repository
            .compact_projection_changes(&failure.change)
            .await
            .unwrap();
        assert!(matches!(
            repository
                .projection_changes(
                    &topology(),
                    &partition(),
                    Some(&created.changes[0].cursor),
                    100,
                )
                .await
                .unwrap(),
            ProjectionChangeRead::ResetRequired { .. }
        ));

        let failed_first = self::repository().await;
        failed_first
            .record_projection_failure(
                ProjectionFailureBatch::new(
                    input(
                        0,
                        b"failed-first",
                        "failed-first-message",
                        "failed-first-cause",
                        ProjectionGeneration::initial(),
                    ),
                    change_epoch(),
                    "failed-first-id",
                    "decode_error",
                    b"bad first payload".to_vec(),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        let repaired_generation = failed_first
            .repair_projection(&topology(), &partition(), "failed-first-id")
            .await
            .unwrap();
        assert!(matches!(
            failed_first
                .commit_projection(batch(
                    input(
                        1,
                        b"later-first",
                        "later-first-message",
                        "later-first-cause",
                        repaired_generation,
                    ),
                    Vec::new(),
                    Vec::new(),
                ))
                .await,
            Err(ProjectionProtocolError::IncomparableInput)
        ));
        let changed_after_repair = TrustedProjectionInput::mint(
            input_cursor(0),
            ProjectionInputFingerprint::from_canonical_bytes(b"retry"),
            "retry-message",
            "retry-cause",
            repaired_generation,
            false,
        )
        .unwrap();
        assert!(matches!(
            failed_first
                .commit_projection(batch(changed_after_repair, Vec::new(), Vec::new()))
                .await,
            Err(ProjectionProtocolError::InputCorruption)
        ));
    }

    #[tokio::test]
    async fn sqlite_registration_rejects_legacy_rows_and_cross_topology_table_owners() {
        let with_legacy_row = unregistered_repository().await;
        assert!(matches!(
            with_legacy_row
                .register_projection_models(&topology(), &[])
                .await,
            Err(ProjectionProtocolError::InvalidBatch(message))
                if message.contains("at least one owned model")
        ));
        with_legacy_row
            .commit_write_plan(TableWritePlan::new(vec![upsert_table_mutation("legacy")]))
            .await
            .unwrap();
        assert!(matches!(
            with_legacy_row
                .register_projection_models(&topology(), &[ownership()])
                .await,
            Err(ProjectionProtocolError::InvalidBatch(message))
                if message.contains("unverified legacy rows")
        ));

        // Deterministically hold the raw-writer fence while bootstrap starts.
        // Raw-first must commit its row; bootstrap then wakes, observes that
        // legacy row under the same fence, and rejects causal ownership.
        let racing = unregistered_repository().await;
        let mut raw_tx = racing.pool().begin().await.unwrap();
        let racing_tables = BTreeSet::from(["sql_todo_views".to_string()]);
        lock_projection_table_ownership_fences_in_tx(&mut raw_tx, &racing_tables)
            .await
            .unwrap();
        let registration_repository = racing.clone();
        let registration = tokio::spawn(async move {
            registration_repository
                .register_projection_models(&topology(), &[ownership()])
                .await
        });
        tokio::task::yield_now().await;
        assert!(!registration.is_finished());
        apply_read_model_write_plan_in_tx(
            &mut raw_tx,
            TableWritePlan::new(vec![upsert_table_mutation("racing-legacy")]),
        )
        .await
        .unwrap();
        raw_tx.commit().await.unwrap();
        assert!(matches!(
            registration.await.unwrap(),
            Err(ProjectionProtocolError::InvalidBatch(message))
                if message.contains("unverified legacy rows")
        ));

        let registered = repository().await;
        let other_topology =
            ProjectorTopologyId::new(1, "other_sql_todo_projector", [99; 32]).unwrap();
        assert!(matches!(
            registered
                .register_projection_models(&other_topology, &[ownership()])
                .await,
            Err(ProjectionProtocolError::InvalidBatch(message))
                if message.contains("authoritatively owned")
        ));
    }

    #[tokio::test]
    async fn sqlite_non_gap_repair_requires_failed_cursor_before_later_input() {
        let repository = repository().await;
        repository
            .commit_projection(batch(
                non_gap_input(
                    5,
                    b"checkpoint-5",
                    "non-gap-message-5",
                    "non-gap-cause-5",
                    ProjectionGeneration::initial(),
                ),
                Vec::new(),
                Vec::new(),
            ))
            .await
            .unwrap();
        repository
            .record_projection_failure(
                ProjectionFailureBatch::new(
                    non_gap_input(
                        9,
                        b"failure-9",
                        "non-gap-message-9",
                        "non-gap-cause-9",
                        ProjectionGeneration::initial(),
                    ),
                    change_epoch(),
                    "non-gap-failure-9",
                    "decode_error",
                    b"bad non-gap payload".to_vec(),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        let generation = repository
            .repair_projection(&topology(), &partition(), "non-gap-failure-9")
            .await
            .unwrap();

        assert_eq!(
            repository
                .commit_projection(batch(
                    non_gap_input(
                        5,
                        b"checkpoint-5",
                        "non-gap-message-5",
                        "non-gap-cause-5",
                        generation,
                    ),
                    Vec::new(),
                    Vec::new(),
                ))
                .await
                .unwrap()
                .outcome,
            ProjectionCommitOutcome::Duplicate
        );
        assert!(matches!(
            repository
                .commit_projection(batch(
                    non_gap_input(
                        10,
                        b"later-10",
                        "non-gap-message-10",
                        "non-gap-cause-10",
                        generation,
                    ),
                    Vec::new(),
                    Vec::new(),
                ))
                .await,
            Err(ProjectionProtocolError::IncomparableInput)
        ));
        assert_eq!(
            repository
                .commit_projection(batch(
                    non_gap_input(
                        9,
                        b"failure-9",
                        "non-gap-message-9",
                        "non-gap-cause-9",
                        generation,
                    ),
                    Vec::new(),
                    Vec::new(),
                ))
                .await
                .unwrap()
                .outcome,
            ProjectionCommitOutcome::Applied
        );
        assert_eq!(
            repository
                .commit_projection(batch(
                    non_gap_input(
                        10,
                        b"later-10",
                        "non-gap-message-10",
                        "non-gap-cause-10",
                        generation,
                    ),
                    Vec::new(),
                    Vec::new(),
                ))
                .await
                .unwrap()
                .outcome,
            ProjectionCommitOutcome::Applied
        );
    }

    #[tokio::test]
    async fn sqlite_record_metadata_is_fenced_against_physical_row_drift() {
        let missing_physical = repository().await;
        let created = missing_physical
            .commit_projection(batch(
                input(
                    1,
                    b"create-for-drift",
                    "drift-message-1",
                    "drift-cause-1",
                    ProjectionGeneration::initial(),
                ),
                vec![mutation(
                    ProjectionRecordExpectation::Missing,
                    ProjectionMutationKind::Upsert,
                )],
                Vec::new(),
            ))
            .await
            .unwrap();
        sqlx::query("DELETE FROM sql_todo_views WHERE id = ?")
            .bind("todo-1")
            .execute(missing_physical.pool())
            .await
            .unwrap();
        assert!(matches!(
            missing_physical
                .commit_projection(batch(
                    input(
                        2,
                        b"update-after-drift",
                        "drift-message-2",
                        "drift-cause-2",
                        ProjectionGeneration::initial(),
                    ),
                    vec![mutation(
                        ProjectionRecordExpectation::Exact(created.records[0].revision.clone()),
                        ProjectionMutationKind::Upsert,
                    )],
                    Vec::new(),
                ))
                .await,
            Err(ProjectionProtocolError::RecordMissing { .. })
        ));
        let direct_missing = SameTransactionProjectionBatch::single_upsert(
            topology(),
            partition(),
            change_epoch(),
            ownership(),
            record_scope(),
            upsert_table_mutation("todo-1"),
            "direct-missing-physical",
        )
        .unwrap();
        let mut tx = missing_physical.pool().begin().await.unwrap();
        assert!(matches!(
            apply_same_transaction_projection_in_tx(
                &mut tx,
                &direct_missing,
                missing_physical.projection_change_retention(),
            )
            .await,
            Err(ProjectionProtocolError::RecordMissing { .. })
        ));
        drop(tx);

        let untracked_physical = repository().await;
        sqlx::query("INSERT INTO sql_todo_views (id, _sourced_version) VALUES (?, ?)")
            .bind("todo-1")
            .bind(1_i64)
            .execute(untracked_physical.pool())
            .await
            .unwrap();
        assert!(matches!(
            untracked_physical
                .commit_projection(batch(
                    input(
                        1,
                        b"claim-untracked",
                        "untracked-message-1",
                        "untracked-cause-1",
                        ProjectionGeneration::initial(),
                    ),
                    vec![mutation(
                        ProjectionRecordExpectation::Missing,
                        ProjectionMutationKind::Upsert,
                    )],
                    Vec::new(),
                ))
                .await,
            Err(ProjectionProtocolError::RecordAlreadyExists { .. })
        ));
        let direct_untracked = SameTransactionProjectionBatch::single_upsert(
            topology(),
            partition(),
            change_epoch(),
            ownership(),
            record_scope(),
            upsert_table_mutation("todo-1"),
            "direct-untracked-physical",
        )
        .unwrap();
        let mut tx = untracked_physical.pool().begin().await.unwrap();
        assert!(matches!(
            apply_same_transaction_projection_in_tx(
                &mut tx,
                &direct_untracked,
                untracked_physical.projection_change_retention(),
            )
            .await,
            Err(ProjectionProtocolError::RecordAlreadyExists { .. })
        ));
    }

    #[tokio::test]
    async fn sqlite_retention_prunes_exact_prefix_and_never_restores_it() {
        let repository = repository_with_retention(2).await;
        let mut cursors = Vec::new();
        for position in 1..=3 {
            let result = repository
                .commit_projection(batch(
                    input(
                        position,
                        format!("retained-{position}").as_bytes(),
                        &format!("retained-message-{position}"),
                        &format!("retained-cause-{position}"),
                        ProjectionGeneration::initial(),
                    ),
                    Vec::new(),
                    Vec::new(),
                ))
                .await
                .unwrap();
            cursors.push(result.changes[0].cursor.clone());
        }
        let failure = repository
            .record_projection_failure(
                ProjectionFailureBatch::new(
                    input(
                        4,
                        b"retained-failure-4",
                        "retained-message-4",
                        "retained-cause-4",
                        ProjectionGeneration::initial(),
                    ),
                    change_epoch(),
                    "retained-failure-4",
                    "decode_error",
                    b"retention failure".to_vec(),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        let retained: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM projection_changes")
            .fetch_one(repository.pool())
            .await
            .unwrap();
        assert_eq!(retained, 2);
        assert!(matches!(
            repository
                .projection_changes(&topology(), &partition(), Some(&cursors[0]), 100)
                .await
                .unwrap(),
            ProjectionChangeRead::ResetRequired {
                compacted_through: 2,
                ..
            }
        ));
        assert!(matches!(
            repository
                .projection_changes(&topology(), &partition(), Some(&cursors[1]), 100)
                .await
                .unwrap(),
            ProjectionChangeRead::Changes {
                compacted_through: 2,
                ref changes,
                ..
            } if changes.len() == 2
                && changes[0].cursor.position() == 3
                && changes[1].cursor == failure.change
        ));

        let repository = repository
            .with_projection_change_retention(ProjectionChangeRetention::new(10).unwrap());
        let generation = repository
            .repair_projection(&topology(), &partition(), "retained-failure-4")
            .await
            .unwrap();
        repository
            .commit_projection(batch(
                input(
                    4,
                    b"retained-failure-4",
                    "retained-message-4",
                    "retained-cause-4",
                    generation,
                ),
                Vec::new(),
                Vec::new(),
            ))
            .await
            .unwrap();
        repository
            .commit_projection(batch(
                input(
                    5,
                    b"retained-5",
                    "retained-message-5",
                    "retained-cause-5",
                    generation,
                ),
                Vec::new(),
                Vec::new(),
            ))
            .await
            .unwrap();
        let retained: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM projection_changes")
            .fetch_one(repository.pool())
            .await
            .unwrap();
        assert_eq!(retained, 4);
        assert!(matches!(
            repository
                .projection_changes(&topology(), &partition(), None, 100)
                .await
                .unwrap(),
            ProjectionChangeRead::ResetRequired {
                compacted_through: 2,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn sqlite_resume_and_concurrent_compaction_share_one_snapshot() {
        let (repository, database_path) = wal_repository_with_retention(16).await;
        let mut cursors = Vec::new();
        for position in 1..=3 {
            let result = repository
                .commit_projection(batch(
                    input(
                        position,
                        format!("resume-{position}").as_bytes(),
                        &format!("resume-message-{position}"),
                        &format!("resume-cause-{position}"),
                        ProjectionGeneration::initial(),
                    ),
                    Vec::new(),
                    Vec::new(),
                ))
                .await
                .unwrap();
            cursors.push(result.changes[0].cursor.clone());
        }

        let reader_pool = repository.pool().clone();
        let resume_after = cursors[0].clone();
        let compact_through = cursors[1].clone();
        let (state_observed_tx, state_observed_rx) = tokio::sync::oneshot::channel();
        let (compaction_committed_tx, compaction_committed_rx) = tokio::sync::oneshot::channel();
        let reader = tokio::spawn(async move {
            read_projection_changes_in_snapshot(
                &reader_pool,
                topology(),
                partition(),
                Some(resume_after),
                100,
                async move {
                    state_observed_tx
                        .send(())
                        .expect("resume reader reports its established state snapshot");
                    compaction_committed_rx
                        .await
                        .expect("compaction completion is reported to resume reader");
                },
            )
            .await
        });

        state_observed_rx
            .await
            .expect("resume reader establishes its snapshot");
        assert_eq!(
            tokio::time::timeout(
                Duration::from_secs(5),
                repository.compact_projection_changes(&compact_through),
            )
            .await
            .expect("WAL compaction commits while the reader snapshot remains open")
            .unwrap(),
            2
        );
        compaction_committed_tx
            .send(())
            .expect("resume reader remains active after concurrent compaction");

        match reader.await.unwrap().unwrap() {
            ProjectionChangeRead::Changes {
                head,
                compacted_through,
                changes,
            } => {
                assert_eq!(head.as_ref().map(ProjectionChangeCursor::position), Some(3));
                assert_eq!(compacted_through, 0);
                assert_eq!(
                    changes
                        .iter()
                        .map(|change| change.cursor.position())
                        .collect::<Vec<_>>(),
                    vec![2, 3],
                    "the established snapshot returns the complete pre-compaction suffix"
                );
            }
            other => panic!("established resume snapshot must return its complete page: {other:?}"),
        }

        assert!(matches!(
            repository
                .projection_changes(&topology(), &partition(), Some(&cursors[0]), 100)
                .await
                .unwrap(),
            ProjectionChangeRead::ResetRequired {
                compacted_through: 2,
                ..
            }
        ));
        assert!(matches!(
            repository
                .projection_changes(&topology(), &partition(), Some(&cursors[1]), 100)
                .await
                .unwrap(),
            ProjectionChangeRead::Changes {
                compacted_through: 2,
                ref changes,
                ..
            } if changes.len() == 1 && changes[0].cursor.position() == 3
        ));

        remove_wal_database(repository, &database_path).await;
    }

    #[tokio::test]
    async fn sqlite_same_transaction_projection_allocates_adapter_evidence() {
        let repository = repository_with_retention(1).await;
        let direct = |causation_id: &str| {
            SameTransactionProjectionBatch::single_upsert(
                topology(),
                partition(),
                change_epoch(),
                ownership(),
                record_scope(),
                upsert_table_mutation("todo-1"),
                causation_id,
            )
            .unwrap()
        };

        let mut tx = repository.pool().begin().await.unwrap();
        let created = apply_same_transaction_projection_in_tx(
            &mut tx,
            &direct("direct-cause-1"),
            repository.projection_change_retention(),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        assert_eq!(created.records[0].revision.incarnation(), 1);
        assert_eq!(created.records[0].revision.revision(), 1);
        assert_eq!(created.changes[0].kind, ProjectionChangeKind::RecordUpsert);
        assert_eq!(
            created.observations[0].revision,
            Some(created.records[0].revision.clone())
        );
        assert!(row_exists(&repository).await);

        let mut tx = repository.pool().begin().await.unwrap();
        let updated = apply_same_transaction_projection_in_tx(
            &mut tx,
            &direct("direct-cause-2"),
            repository.projection_change_retention(),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        assert_eq!(updated.records[0].revision.incarnation(), 1);
        assert_eq!(updated.records[0].revision.revision(), 2);
        assert_eq!(
            updated.changes[0].cursor.position(),
            created.changes[0].cursor.position() + 1
        );
        assert_eq!(
            repository
                .projection_observation(
                    "direct-cause-2",
                    &record_scope(),
                    ProjectionObservationKind::Record,
                )
                .await
                .unwrap()
                .unwrap(),
            updated.observations[0]
        );
        let retained: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM projection_changes")
            .fetch_one(repository.pool())
            .await
            .unwrap();
        assert_eq!(retained, 1);
        assert!(matches!(
            repository
                .projection_changes(&topology(), &partition(), None, 100)
                .await
                .unwrap(),
            ProjectionChangeRead::ResetRequired {
                compacted_through: 1,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn sqlite_direct_projection_and_ledger_replay_commit_atomically() {
        let repository = repository().await;
        let command_id = uuid::Uuid::now_v7().hyphenated().to_string();
        let key = CommandLedgerKey::new(
            "projection-runtime-test",
            PrincipalPartitionId::new("tenant:direct").unwrap(),
            CommandId::parse(command_id).unwrap(),
        )
        .unwrap();
        let retention = Duration::from_secs(3600);
        let reservation = CommandReservation::new(
            key.clone(),
            "project-todo",
            CommandContractFingerprint::new([51; 32]),
            CanonicalInputHash::new([52; 32]),
            Duration::from_secs(30),
            retention,
        )
        .unwrap();
        let attempt = match repository.reserve_command(reservation).await.unwrap() {
            ReservationOutcome::Acquired(attempt) => attempt,
            _ => panic!("fresh command reservation must acquire its first attempt"),
        };
        let causation_id = attempt.causation_id().as_str().to_string();
        let completion = attempt
            .complete(
                TerminalCommandState::Projected,
                serde_json::json!({"projected": true}),
                retention,
            )
            .unwrap();
        let direct = SameTransactionProjectionBatch::single_upsert(
            topology(),
            partition(),
            change_epoch(),
            ownership(),
            record_scope(),
            upsert_table_mutation("todo-1"),
            causation_id.as_str(),
        )
        .unwrap();

        repository
            .commit_causal_batch(CausalCommitBatch::with_direct_projection(
                CommitBatch::empty(),
                completion,
                direct,
            ))
            .await
            .unwrap();

        let metadata = repository
            .projection_record(&record_scope())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(metadata.revision.revision(), 1);
        assert!(row_exists(&repository).await);
        match repository
            .lookup_command(&key, CommandLookupScope::CommandName("project-todo"))
            .await
            .unwrap()
        {
            CommandLookup::Replay(replay) => {
                assert_eq!(replay.outcome, serde_json::json!({"projected": true}));
                assert!(replay.direct_projection.is_some());
            }
            _ => panic!("completed direct projection must replay its exact evidence"),
        }

        let failed_key = CommandLedgerKey::new(
            "projection-runtime-test",
            PrincipalPartitionId::new("tenant:direct").unwrap(),
            CommandId::parse(uuid::Uuid::now_v7().hyphenated().to_string()).unwrap(),
        )
        .unwrap();
        let failed_reservation = CommandReservation::new(
            failed_key.clone(),
            "project-todo",
            CommandContractFingerprint::new([61; 32]),
            CanonicalInputHash::new([62; 32]),
            Duration::from_secs(30),
            retention,
        )
        .unwrap();
        let failed_attempt = match repository
            .reserve_command(failed_reservation)
            .await
            .unwrap()
        {
            ReservationOutcome::Acquired(attempt) => attempt,
            _ => panic!("fresh rollback reservation must acquire its first attempt"),
        };
        let failed_causation = failed_attempt.causation_id().as_str().to_string();
        let failed_completion = failed_attempt
            .complete(
                TerminalCommandState::Projected,
                serde_json::json!({"projected": "must-roll-back"}),
                retention,
            )
            .unwrap();
        let failed_direct = SameTransactionProjectionBatch::single_upsert(
            topology(),
            partition(),
            change_epoch(),
            ownership(),
            record_scope(),
            upsert_table_mutation("todo-1"),
            failed_causation,
        )
        .unwrap();
        sqlx::query(
            "CREATE TRIGGER fail_direct_ledger_completion \
             BEFORE UPDATE OF state ON command_ledger \
             WHEN NEW.state = 'projected' \
             BEGIN SELECT RAISE(ABORT, 'forced direct ledger failure'); END",
        )
        .execute(repository.pool())
        .await
        .unwrap();
        assert!(repository
            .commit_causal_batch(CausalCommitBatch::with_direct_projection(
                CommitBatch::empty(),
                failed_completion,
                failed_direct,
            ))
            .await
            .is_err());
        assert_eq!(
            repository
                .projection_record(&record_scope())
                .await
                .unwrap()
                .unwrap()
                .revision
                .revision(),
            1
        );
        assert!(matches!(
            repository
                .lookup_command(&failed_key, CommandLookupScope::CommandName("project-todo"),)
                .await
                .unwrap(),
            CommandLookup::InProgress { .. }
        ));
        sqlx::query("DROP TRIGGER fail_direct_ledger_completion")
            .execute(repository.pool())
            .await
            .unwrap();

        let fenced_key = CommandLedgerKey::new(
            "projection-runtime-test",
            PrincipalPartitionId::new("tenant:direct").unwrap(),
            CommandId::parse(uuid::Uuid::now_v7().hyphenated().to_string()).unwrap(),
        )
        .unwrap();
        let fenced_reservation = CommandReservation::new(
            fenced_key,
            "project-todo",
            CommandContractFingerprint::new([71; 32]),
            CanonicalInputHash::new([72; 32]),
            Duration::from_secs(30),
            retention,
        )
        .unwrap();
        let fenced_attempt = match repository
            .reserve_command(fenced_reservation)
            .await
            .unwrap()
        {
            ReservationOutcome::Acquired(attempt) => attempt,
            _ => panic!("fresh fenced reservation must acquire its first attempt"),
        };
        let fenced_causation = fenced_attempt.causation_id().as_str().to_string();
        let fenced_completion = fenced_attempt
            .complete(
                TerminalCommandState::Projected,
                serde_json::json!({"projected": "must-not-run"}),
                retention,
            )
            .unwrap();
        repository
            .mark_retryable_unknown(fenced_completion.attempt_fence())
            .await
            .unwrap();
        let fenced_direct = SameTransactionProjectionBatch::single_upsert(
            topology(),
            partition(),
            change_epoch(),
            ownership(),
            record_scope(),
            upsert_table_mutation("todo-1"),
            fenced_causation,
        )
        .unwrap();
        sqlx::query(
            "CREATE TRIGGER fail_if_fenced_projection_runs \
             BEFORE UPDATE ON sql_todo_views \
             BEGIN SELECT RAISE(ABORT, 'fenced direct projection executed'); END",
        )
        .execute(repository.pool())
        .await
        .unwrap();
        assert!(matches!(
            repository
                .commit_causal_batch(CausalCommitBatch::with_direct_projection(
                    CommitBatch::empty(),
                    fenced_completion,
                    fenced_direct,
                ))
                .await,
            Err(CommandLedgerError::AttemptFenced { .. })
        ));
        assert_eq!(
            repository
                .projection_record(&record_scope())
                .await
                .unwrap()
                .unwrap()
                .revision
                .revision(),
            1
        );
        sqlx::query("DROP TRIGGER fail_if_fenced_projection_runs")
            .execute(repository.pool())
            .await
            .unwrap();
    }
}

#[cfg(all(test, feature = "postgres"))]
mod postgres_tests {
    use std::sync::LazyLock;
    use std::time::Duration;

    use super::*;
    use crate::command_ledger::{
        CanonicalInputHash, CausalCommitBatch, CausalTransactionalCommit,
        CommandContractFingerprint, CommandId, CommandLedgerKey, CommandLedgerStore,
        CommandReservation, PrincipalPartitionId, ReservationOutcome, TerminalCommandState,
    };
    use crate::projection_protocol::{
        ProjectionCheckpointProbe, ProjectionRecordMutation, ProjectionScopeCodec,
    };
    use crate::repository::{CommitBatch, ReadModelWritePlanStore};
    use crate::table::{
        ColumnType, DeleteTableRowMutation, ExpectedVersion, PrimaryKey, RowKey, RowValue,
        RowValues, RowWriteMode, TableColumn, TableKind, TableRowMutation, TableSchema,
        TableSchemaRegistry, TableStoreError, TableWritePlan,
    };

    fn topology() -> ProjectorTopologyId {
        ProjectorTopologyId::new(1, "postgres_projection_runtime", [71; 32]).unwrap()
    }

    fn partition() -> ProjectionPartition {
        ProjectionScopeCodec::new(topology())
            .encode_partition(Some(&serde_json::json!("postgres-runtime-tenant")))
            .unwrap()
    }

    fn change_epoch() -> ProjectionEpoch {
        ProjectionEpoch::new("postgres-changes-v1").unwrap()
    }

    fn schema() -> &'static TableSchema {
        static SCHEMA: LazyLock<TableSchema> = LazyLock::new(|| TableSchema {
            model_name: "PostgresProjectionView".into(),
            table_name: "postgres_projection_views".into(),
            columns: vec![
                TableColumn {
                    primary_key: true,
                    ..TableColumn::new("id", "id", ColumnType::Text)
                },
                TableColumn::new("value", "value", ColumnType::Text),
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

    fn ownership() -> ProjectionModelOwnership {
        ProjectionModelOwnership::new("PostgresProjectionView", "postgres_projection_views")
            .unwrap()
    }

    fn scope_codec() -> ProjectionScopeCodec {
        ProjectionScopeCodec::with_models(topology(), [("PostgresProjectionView", schema())])
            .unwrap()
    }

    fn record_key() -> RowKey {
        RowKey::new([("id", RowValue::String("runtime-row".into()))])
    }

    fn scope() -> ProjectionRecordScope {
        scope_codec()
            .encode_row_scope(
                "postgres_projection_runtime",
                "PostgresProjectionView",
                Some(&serde_json::json!("postgres-runtime-tenant")),
                &record_key(),
            )
            .unwrap()
    }

    fn source() -> ProjectionSource {
        ProjectionSource::new("postgres_runtime_source", b"runtime-row".to_vec()).unwrap()
    }

    fn snapshot_request(generation: ProjectionGeneration) -> ProjectionQuerySnapshotRequest {
        ProjectionQuerySnapshotRequest::new(
            &scope_codec(),
            Some(&serde_json::json!("postgres-runtime-tenant")),
            "PostgresProjectionView",
            record_key(),
            vec![ProjectionCheckpointProbe::new(
                topology(),
                partition(),
                source(),
                ProjectionEpoch::new("postgres-source-v1").unwrap(),
                generation,
            )],
        )
        .unwrap()
    }

    fn input(
        position: u64,
        fingerprint: &[u8],
        message_id: &str,
        causation_id: &str,
        generation: ProjectionGeneration,
    ) -> TrustedProjectionInput {
        TrustedProjectionInput::mint(
            ProjectionInputCursor::new(
                topology(),
                partition(),
                source(),
                ProjectionEpoch::new("postgres-source-v1").unwrap(),
                position,
            )
            .unwrap(),
            ProjectionInputFingerprint::from_canonical_bytes(fingerprint),
            message_id,
            causation_id,
            generation,
            false,
        )
        .unwrap()
    }

    fn upsert_table_mutation(value: &str) -> TableMutation {
        let mut values = RowValues::new();
        values.insert("id", RowValue::String("runtime-row".into()));
        values.insert("value", RowValue::String(value.into()));
        TableMutation::UpsertRow(TableRowMutation {
            schema: schema(),
            key: record_key(),
            values,
            expected_version: ExpectedVersion::Any,
            mode: RowWriteMode::Upsert,
        })
    }

    fn upsert_mutation(
        value: &str,
        expectation: ProjectionRecordExpectation,
        kind: ProjectionMutationKind,
    ) -> ProjectionRecordMutation {
        ProjectionRecordMutation::new(scope(), upsert_table_mutation(value), expectation, kind)
            .unwrap()
    }

    fn delete_mutation(expected: RecordRevision) -> ProjectionRecordMutation {
        ProjectionRecordMutation::new(
            scope(),
            TableMutation::DeleteRow(DeleteTableRowMutation {
                schema: schema(),
                key: record_key(),
                expected_version: ExpectedVersion::Any,
            }),
            ProjectionRecordExpectation::Exact(expected),
            ProjectionMutationKind::Delete,
        )
        .unwrap()
    }

    fn batch(
        input: TrustedProjectionInput,
        mutations: Vec<ProjectionRecordMutation>,
    ) -> ProjectionCommitBatch {
        ProjectionCommitBatch {
            input,
            change_epoch: change_epoch(),
            ownership: vec![ownership()],
            mutations,
            observations: Vec::new(),
        }
    }

    fn assert_live_snapshot(
        snapshot: &ProjectionQuerySnapshot,
        value: &str,
        incarnation: u64,
        revision: u64,
        source_position: u64,
    ) {
        assert_eq!(
            snapshot.row.as_ref().and_then(|row| row.get("value")),
            Some(&RowValue::String(value.into()))
        );
        let record = snapshot.record.as_ref().expect("live record metadata");
        assert!(!record.tombstone);
        assert_eq!(record.revision.incarnation(), incarnation);
        assert_eq!(record.revision.revision(), revision);
        let checkpoint = snapshot.checkpoints[0]
            .checkpoint
            .as_ref()
            .expect("explicit source checkpoint");
        assert_eq!(checkpoint.input().position(), source_position);
        assert_eq!(checkpoint.change(), &record.change);
        assert_eq!(snapshot.change_head.as_ref(), Some(&record.change));
    }

    #[tokio::test]
    async fn postgres_projection_protocol_runtime_conforms() {
        let Ok(database_url) = std::env::var("DISTRIBUTED_TEST_POSTGRES_URL") else {
            return;
        };
        let repository = SqlxRepository::<sqlx::Postgres>::connect_and_migrate(&database_url)
            .await
            .unwrap()
            .with_projection_change_retention(ProjectionChangeRetention::new(3).unwrap());
        let mut registry = TableSchemaRegistry::new();
        registry.register_schema(schema().clone()).unwrap();
        repository
            .bootstrap_table_schema_for_dev(&registry)
            .await
            .unwrap();

        let mut raw_first_tx = repository.pool().begin().await.unwrap();
        let ownership_tables = BTreeSet::from(["postgres_projection_views".to_string()]);
        lock_projection_table_ownership_fences_in_tx(&mut raw_first_tx, &ownership_tables)
            .await
            .unwrap();
        let registration_repository = repository.clone();
        let racing_registration = tokio::spawn(async move {
            registration_repository
                .register_projection_models(&topology(), &[ownership()])
                .await
        });
        tokio::task::yield_now().await;
        assert!(!racing_registration.is_finished());
        apply_read_model_write_plan_in_tx(
            &mut raw_first_tx,
            TableWritePlan::new(vec![upsert_table_mutation("raw-first")]),
        )
        .await
        .unwrap();
        raw_first_tx.commit().await.unwrap();
        assert!(matches!(
            racing_registration.await.unwrap(),
            Err(ProjectionProtocolError::InvalidBatch(message))
                if message.contains("unverified legacy rows")
        ));
        sqlx::query("DELETE FROM postgres_projection_views WHERE id = $1")
            .bind("runtime-row")
            .execute(repository.pool())
            .await
            .unwrap();
        repository
            .register_projection_models(&topology(), &[ownership()])
            .await
            .unwrap();

        assert_eq!(
            repository
                .projection_partition_runtime_state(&topology(), &partition())
                .await
                .unwrap(),
            None
        );
        assert!(matches!(
            repository
                .commit_write_plan(TableWritePlan::new(vec![upsert_table_mutation("raw")]))
                .await,
            Err(TableStoreError::CausalWriteRequired { ref table })
                if table == "postgres_projection_views"
        ));

        let mut local_changes = repository.read_model_changes();
        let mut pg_changes = sqlx::postgres::PgListener::connect(&database_url)
            .await
            .unwrap();
        pg_changes
            .listen("distributed_read_model_changes")
            .await
            .unwrap();
        let initial_input = input(
            5,
            b"postgres-checkpoint-5",
            "postgres-message-5",
            "postgres-cause-5",
            ProjectionGeneration::initial(),
        );
        assert_eq!(
            repository
                .projection_input_disposition(&initial_input)
                .await
                .unwrap(),
            ProjectionInputDisposition::Pending
        );
        let mut initial = batch(
            initial_input.clone(),
            vec![upsert_mutation(
                "5",
                ProjectionRecordExpectation::Missing,
                ProjectionMutationKind::Upsert,
            )],
        );
        initial.observations = vec![crate::projection_protocol::ProjectionObservationRequest {
            kind: ProjectionObservationKind::Record,
            target: ProjectionObservationTarget::StagedRecord(scope()),
        }];
        assert_eq!(
            repository.commit_projection(initial).await.unwrap().outcome,
            ProjectionCommitOutcome::Applied
        );
        assert!(matches!(
            repository
                .projection_input_disposition(&initial_input)
                .await
                .unwrap(),
            ProjectionInputDisposition::Duplicate(checkpoint)
                if checkpoint.input().position() == 5
        ));
        let local_change = tokio::time::timeout(Duration::from_secs(2), local_changes.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(local_change.tables.contains(PROJECTION_CHANGE_NOTIFY_TABLE));
        tokio::time::timeout(Duration::from_secs(2), pg_changes.recv())
            .await
            .expect("PostgreSQL notifies only after the projection transaction commits")
            .unwrap();
        assert_live_snapshot(
            &repository
                .projection_query_snapshot(&snapshot_request(ProjectionGeneration::initial()))
                .await
                .unwrap(),
            "5",
            1,
            1,
            5,
        );
        let causal_probe = crate::projection_protocol::ProjectionObligationEvidenceRequest::new(
            "postgres-cause-5",
            scope(),
            ProjectionObservationKind::Record,
        )
        .unwrap();
        assert!(matches!(
            repository
                .projection_obligation_evidence_batch(
                    &ProjectionObligationEvidenceBatchRequest::new(vec![causal_probe.clone()])
                        .unwrap(),
                )
                .await
                .unwrap()
                .evidence
                .as_slice(),
            [ProjectionObligationEvidence::Observed(_)]
        ));

        assert_eq!(
            repository
                .commit_projection(batch(
                    input(
                        5,
                        b"postgres-checkpoint-5",
                        "postgres-message-5",
                        "postgres-cause-5",
                        ProjectionGeneration::initial(),
                    ),
                    vec![upsert_mutation(
                        "ignored-duplicate",
                        ProjectionRecordExpectation::Missing,
                        ProjectionMutationKind::Upsert,
                    )],
                ))
                .await
                .unwrap()
                .outcome,
            ProjectionCommitOutcome::Duplicate
        );
        assert!(matches!(
            repository
                .commit_projection(batch(
                    input(
                        5,
                        b"postgres-corrupt-5",
                        "postgres-message-5",
                        "postgres-cause-5",
                        ProjectionGeneration::initial(),
                    ),
                    Vec::new(),
                ))
                .await,
            Err(ProjectionProtocolError::InputCorruption)
        ));
        let remapped_partition = ProjectionScopeCodec::new(topology())
            .encode_partition(Some(&serde_json::json!("postgres-other-tenant")))
            .unwrap();
        let remapped_message = TrustedProjectionInput::mint(
            ProjectionInputCursor::new(
                topology(),
                remapped_partition,
                source(),
                ProjectionEpoch::new("postgres-source-v1").unwrap(),
                5,
            )
            .unwrap(),
            ProjectionInputFingerprint::from_canonical_bytes(b"postgres-checkpoint-5"),
            "postgres-message-5",
            "postgres-cause-5",
            ProjectionGeneration::initial(),
            false,
        )
        .unwrap();
        assert!(matches!(
            repository
                .commit_projection(batch(remapped_message, Vec::new()))
                .await,
            Err(ProjectionProtocolError::MessageIdReuse { message_id })
                if message_id == "postgres-message-5"
        ));
        let stale_input = input(
            4,
            b"postgres-stale-4",
            "postgres-message-4",
            "postgres-cause-4",
            ProjectionGeneration::initial(),
        );
        assert_eq!(
            repository
                .commit_projection(batch(stale_input.clone(), Vec::new()))
                .await
                .unwrap()
                .outcome,
            ProjectionCommitOutcome::StaleInput
        );
        assert!(matches!(
            repository
                .projection_input_disposition(&stale_input)
                .await
                .unwrap(),
            ProjectionInputDisposition::Stale(checkpoint)
                if checkpoint.input().position() == 5
        ));

        let created = repository
            .projection_record(&scope())
            .await
            .unwrap()
            .unwrap();
        let deleted = repository
            .commit_projection(batch(
                input(
                    6,
                    b"postgres-delete-6",
                    "postgres-message-6",
                    "postgres-cause-6",
                    ProjectionGeneration::initial(),
                ),
                vec![delete_mutation(created.revision)],
            ))
            .await
            .unwrap();
        assert!(deleted.records[0].tombstone);
        assert!(repository
            .projection_query_snapshot(&snapshot_request(ProjectionGeneration::initial()))
            .await
            .unwrap()
            .row
            .is_none());
        let recreated = repository
            .commit_projection(batch(
                input(
                    7,
                    b"postgres-recreate-7",
                    "postgres-message-7",
                    "postgres-cause-7",
                    ProjectionGeneration::initial(),
                ),
                vec![upsert_mutation(
                    "7",
                    ProjectionRecordExpectation::Exact(deleted.records[0].revision.clone()),
                    ProjectionMutationKind::Recreate,
                )],
            ))
            .await
            .unwrap();
        assert_eq!(recreated.records[0].revision.incarnation(), 2);
        assert_eq!(recreated.records[0].revision.revision(), 1);

        let failed_input = input(
            9,
            b"postgres-failure-9",
            "postgres-message-9",
            "postgres-cause-5",
            ProjectionGeneration::initial(),
        );
        let failure = repository
            .record_projection_failure(
                ProjectionFailureBatch::new(
                    failed_input.clone(),
                    change_epoch(),
                    "postgres-failure-9",
                    "decode_error",
                    b"bad postgres payload".to_vec(),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        assert!(matches!(
            repository
                .projection_input_disposition(&failed_input)
                .await,
            Err(ProjectionProtocolError::PartitionStopped { failure_id })
                if failure_id == "postgres-failure-9"
        ));
        assert!(!failure.gap_free);
        let stopped = repository
            .projection_partition_runtime_state(&topology(), &partition())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stopped.active_generation, ProjectionGeneration::initial());
        assert_eq!(
            stopped.stopped_failure_id.as_deref(),
            Some("postgres-failure-9")
        );
        assert_eq!(stopped.pending_retry, None);
        let generation = repository
            .repair_projection(&topology(), &partition(), "postgres-failure-9")
            .await
            .unwrap();
        let retry_input = input(
            9,
            b"postgres-failure-9",
            "postgres-message-9",
            "postgres-cause-5",
            generation,
        );
        assert_eq!(
            repository
                .projection_input_disposition(&retry_input)
                .await
                .unwrap(),
            ProjectionInputDisposition::Pending
        );
        let repaired = repository
            .projection_partition_runtime_state(&topology(), &partition())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(repaired.active_generation, generation);
        assert_eq!(repaired.stopped_failure_id, None);
        let pending_retry = repaired.pending_retry.unwrap();
        assert_eq!(pending_retry.failure_id, failure.failure_id);
        assert_eq!(pending_retry.input, failure.input);
        assert_eq!(pending_retry.input_fingerprint, failure.input_fingerprint);
        assert_eq!(pending_retry.message_id, failure.message_id);
        assert_eq!(pending_retry.causation_id, failure.causation_id);
        assert_eq!(pending_retry.failed_generation, failure.generation);
        assert_eq!(pending_retry.gap_free, failure.gap_free);
        assert!(matches!(
            repository
                .commit_projection(batch(
                    input(
                        10,
                        b"postgres-later-10",
                        "postgres-message-10",
                        "postgres-cause-10",
                        generation,
                    ),
                    vec![upsert_mutation(
                        "must-not-run-before-retry",
                        ProjectionRecordExpectation::Exact(recreated.records[0].revision.clone(),),
                        ProjectionMutationKind::Upsert,
                    )],
                ))
                .await,
            Err(ProjectionProtocolError::IncomparableInput)
        ));
        assert_eq!(
            repository
                .commit_projection(batch(retry_input, Vec::new()))
                .await
                .unwrap()
                .outcome,
            ProjectionCommitOutcome::Applied
        );
        assert_eq!(
            repository
                .commit_projection(batch(
                    input(
                        10,
                        b"postgres-later-10",
                        "postgres-message-10",
                        "postgres-cause-10",
                        generation,
                    ),
                    vec![upsert_mutation(
                        "10",
                        ProjectionRecordExpectation::Exact(recreated.records[0].revision.clone()),
                        ProjectionMutationKind::Upsert,
                    )],
                ))
                .await
                .unwrap()
                .outcome,
            ProjectionCommitOutcome::Applied
        );

        let before_rollback = repository
            .projection_query_snapshot(&snapshot_request(generation))
            .await
            .unwrap();
        assert_live_snapshot(&before_rollback, "10", 2, 2, 10);
        let before_rollback_inbox: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM consumer_inbox")
            .fetch_one(repository.pool())
            .await
            .unwrap();
        sqlx::query(
            "CREATE OR REPLACE FUNCTION fail_projection_record_write() RETURNS trigger \
             LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'forced projection record failure'; \
             END; $$",
        )
        .execute(repository.pool())
        .await
        .unwrap();
        sqlx::query(
            "CREATE TRIGGER fail_projection_record_write \
             BEFORE INSERT OR UPDATE ON projection_records FOR EACH ROW \
             EXECUTE FUNCTION fail_projection_record_write()",
        )
        .execute(repository.pool())
        .await
        .unwrap();
        let mut rollback_local_changes = repository.read_model_changes();
        let mut rollback_pg_changes = sqlx::postgres::PgListener::connect(&database_url)
            .await
            .unwrap();
        rollback_pg_changes
            .listen("distributed_read_model_changes")
            .await
            .unwrap();
        assert!(repository
            .commit_projection(batch(
                input(
                    11,
                    b"postgres-rollback-11",
                    "postgres-message-11",
                    "postgres-cause-11",
                    generation,
                ),
                vec![upsert_mutation(
                    "must-roll-back",
                    ProjectionRecordExpectation::Exact(
                        before_rollback.record.as_ref().unwrap().revision.clone(),
                    ),
                    ProjectionMutationKind::Upsert,
                )],
            ))
            .await
            .is_err());
        assert_eq!(
            rollback_local_changes.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(150), rollback_pg_changes.recv())
                .await
                .is_err(),
            "a rolled-back pg_notify must never be delivered"
        );
        assert_eq!(
            repository
                .projection_query_snapshot(&snapshot_request(generation))
                .await
                .unwrap(),
            before_rollback
        );
        let after_rollback_inbox: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM consumer_inbox")
            .fetch_one(repository.pool())
            .await
            .unwrap();
        assert_eq!(after_rollback_inbox, before_rollback_inbox);
        let rolled_back_receipt: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM projection_input_receipts WHERE message_id = $1",
        )
        .bind("postgres-message-11")
        .fetch_one(repository.pool())
        .await
        .unwrap();
        assert_eq!(rolled_back_receipt, 0);
        sqlx::query("DROP TRIGGER fail_projection_record_write ON projection_records")
            .execute(repository.pool())
            .await
            .unwrap();
        sqlx::query("DROP FUNCTION fail_projection_record_write()")
            .execute(repository.pool())
            .await
            .unwrap();

        let successful_11 = repository
            .commit_projection(batch(
                input(
                    11,
                    b"postgres-rollback-11",
                    "postgres-message-11",
                    "postgres-cause-11",
                    generation,
                ),
                vec![upsert_mutation(
                    "11",
                    ProjectionRecordExpectation::Exact(
                        before_rollback.record.as_ref().unwrap().revision.clone(),
                    ),
                    ProjectionMutationKind::Upsert,
                )],
            ))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), rollback_local_changes.recv())
            .await
            .expect("local change is published after commit")
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), rollback_pg_changes.recv())
            .await
            .expect("PostgreSQL notification is delivered after commit")
            .unwrap();
        let committed_11 = repository
            .projection_query_snapshot(&snapshot_request(generation))
            .await
            .unwrap();
        assert_live_snapshot(&committed_11, "11", 2, 3, 11);

        let repeatable_request = snapshot_request(generation);
        let repeatable_live_request = ProjectionLiveRecordBatchRequest::new(vec![
            crate::projection_protocol::ProjectionLiveRecordRequest::new(
                &scope_codec(),
                "PostgresProjectionView",
                record_key(),
            )
            .unwrap(),
        ])
        .unwrap();
        let repeatable_expectation = successful_11.records[0].revision.clone();
        let writer_repository = repository.clone();
        let (inside_before, inside_live_before, inside_after, inside_live_after) =
            with_projection_read_snapshot(repository.pool(), move |connection| {
                Box::pin(async move {
                    let before = read_projection_query_snapshot_in_executor::<sqlx::Postgres, _>(
                        &mut *connection,
                        &repeatable_request,
                    )
                    .await?;
                    let live_before =
                        read_projection_live_record_batch_in_executor::<sqlx::Postgres>(
                            &mut *connection,
                            &repeatable_live_request,
                        )
                        .await?;
                    writer_repository
                        .commit_projection(batch(
                            input(
                                12,
                                b"postgres-repeatable-12",
                                "postgres-message-12",
                                "postgres-cause-12",
                                generation,
                            ),
                            vec![upsert_mutation(
                                "12",
                                ProjectionRecordExpectation::Exact(repeatable_expectation),
                                ProjectionMutationKind::Upsert,
                            )],
                        ))
                        .await?;
                    let after = read_projection_query_snapshot_in_executor::<sqlx::Postgres, _>(
                        &mut *connection,
                        &repeatable_request,
                    )
                    .await?;
                    let live_after =
                        read_projection_live_record_batch_in_executor::<sqlx::Postgres>(
                            &mut *connection,
                            &repeatable_live_request,
                        )
                        .await?;
                    Ok((before, live_before, after, live_after))
                })
            })
            .await
            .unwrap();
        assert_eq!(inside_after, inside_before);
        assert_eq!(inside_live_after, inside_live_before);
        assert_eq!(
            inside_live_before.records[0], inside_before.record,
            "physical query and magic live-record evidence share one repeatable snapshot"
        );
        assert_live_snapshot(&inside_before, "11", 2, 3, 11);
        let committed_12 = repository
            .projection_query_snapshot(&snapshot_request(generation))
            .await
            .unwrap();
        assert_live_snapshot(&committed_12, "12", 2, 4, 12);
        let committed_live = repository
            .projection_live_record_batch(
                &ProjectionLiveRecordBatchRequest::new(vec![
                    crate::projection_protocol::ProjectionLiveRecordRequest::new(
                        &scope_codec(),
                        "PostgresProjectionView",
                        record_key(),
                    )
                    .unwrap(),
                ])
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(committed_live.records[0], committed_12.record);

        let retained_failure_change: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM projection_changes WHERE failure_id = $1")
                .bind(failure.failure_id.as_str())
                .fetch_one(repository.pool())
                .await
                .unwrap();
        assert_eq!(
            retained_failure_change, 0,
            "bounded retention must compact the failure change before evidence lookup"
        );
        assert!(matches!(
            repository
                .projection_obligation_evidence_batch(
                    &ProjectionObligationEvidenceBatchRequest::new(vec![causal_probe]).unwrap(),
                )
                .await
                .unwrap()
                .evidence
                .as_slice(),
            [ProjectionObligationEvidence::TerminalFailure(stored)]
                if stored == &failure
        ));

        let physical_version: i64 = sqlx::query_scalar(
            "SELECT _sourced_version FROM postgres_projection_views WHERE id = $1",
        )
        .bind("runtime-row")
        .fetch_one(repository.pool())
        .await
        .unwrap();
        sqlx::query("DELETE FROM postgres_projection_views WHERE id = $1")
            .bind("runtime-row")
            .execute(repository.pool())
            .await
            .unwrap();
        assert!(matches!(
            repository
                .projection_query_snapshot(&snapshot_request(generation))
                .await,
            Err(ProjectionProtocolError::RecordMissing { .. })
        ));
        sqlx::query(
            "INSERT INTO postgres_projection_views (id, value, _sourced_version) \
             VALUES ($1, $2, $3)",
        )
        .bind("runtime-row")
        .bind("12")
        .bind(physical_version)
        .execute(repository.pool())
        .await
        .unwrap();
        assert_eq!(
            repository
                .projection_query_snapshot(&snapshot_request(generation))
                .await
                .unwrap(),
            committed_12
        );

        let retention = Duration::from_secs(3600);
        let reservation = CommandReservation::new(
            CommandLedgerKey::new(
                "postgres-projection-runtime",
                PrincipalPartitionId::new("tenant:postgres-direct").unwrap(),
                CommandId::parse(uuid::Uuid::now_v7().hyphenated().to_string()).unwrap(),
            )
            .unwrap(),
            "postgres-project-view",
            CommandContractFingerprint::new([81; 32]),
            CanonicalInputHash::new([82; 32]),
            Duration::from_secs(30),
            retention,
        )
        .unwrap();
        let attempt = match repository.reserve_command(reservation).await.unwrap() {
            ReservationOutcome::Acquired(attempt) => attempt,
            _ => panic!("fresh PostgreSQL command must acquire its first attempt"),
        };
        let causation_id = attempt.causation_id().as_str().to_string();
        let completion = attempt
            .complete(
                TerminalCommandState::Projected,
                serde_json::json!({"postgres_projected": true}),
                retention,
            )
            .unwrap();
        let direct = SameTransactionProjectionBatch::single_upsert(
            topology(),
            partition(),
            change_epoch(),
            ownership(),
            scope(),
            upsert_table_mutation("direct"),
            causation_id,
        )
        .unwrap();
        repository
            .commit_causal_batch(CausalCommitBatch::with_direct_projection(
                CommitBatch::empty(),
                completion,
                direct,
            ))
            .await
            .unwrap();
        let after_direct = repository
            .projection_record(&scope())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(after_direct.revision.incarnation(), 2);
        assert_eq!(after_direct.revision.revision(), 5);

        let (head, compacted_through) = match repository
            .projection_changes(&topology(), &partition(), None, 100)
            .await
            .unwrap()
        {
            ProjectionChangeRead::ResetRequired {
                head,
                compacted_through,
            } => (head.unwrap(), compacted_through),
            other => panic!("retention must require reset from origin: {other:?}"),
        };
        assert!(compacted_through > 0);
        assert_eq!(head.position(), compacted_through + 3);
        let boundary =
            ProjectionChangeCursor::new(topology(), partition(), change_epoch(), compacted_through)
                .unwrap();
        match repository
            .projection_changes(&topology(), &partition(), Some(&boundary), 100)
            .await
            .unwrap()
        {
            ProjectionChangeRead::Changes {
                head: retained_head,
                compacted_through: retained_watermark,
                changes,
            } => {
                assert_eq!(retained_head, Some(head));
                assert_eq!(retained_watermark, compacted_through);
                assert_eq!(changes.len(), 3);
                assert_eq!(changes[0].cursor.position(), compacted_through + 1);
            }
            other => panic!("exact compacted boundary must resume retained suffix: {other:?}"),
        }

        let resume_after = ProjectionChangeCursor::new(
            topology(),
            partition(),
            change_epoch(),
            compacted_through + 1,
        )
        .unwrap();
        let compact_through = ProjectionChangeCursor::new(
            topology(),
            partition(),
            change_epoch(),
            compacted_through + 2,
        )
        .unwrap();
        let reader_pool = repository.pool().clone();
        let raced_resume_after = resume_after.clone();
        let (state_observed_tx, state_observed_rx) = tokio::sync::oneshot::channel();
        let (compaction_committed_tx, compaction_committed_rx) = tokio::sync::oneshot::channel();
        let reader = tokio::spawn(async move {
            read_projection_changes_in_snapshot(
                &reader_pool,
                topology(),
                partition(),
                Some(raced_resume_after),
                100,
                async move {
                    state_observed_tx
                        .send(())
                        .expect("PostgreSQL resume reader reports its established snapshot");
                    compaction_committed_rx
                        .await
                        .expect("PostgreSQL compaction completion reaches resume reader");
                },
            )
            .await
        });

        state_observed_rx
            .await
            .expect("PostgreSQL resume reader establishes its snapshot");
        assert_eq!(
            tokio::time::timeout(
                Duration::from_secs(5),
                repository.compact_projection_changes(&compact_through),
            )
            .await
            .expect("PostgreSQL compaction commits while repeatable reader remains open")
            .unwrap(),
            compacted_through + 2
        );
        compaction_committed_tx
            .send(())
            .expect("PostgreSQL resume reader remains active after compaction");

        match reader.await.unwrap().unwrap() {
            ProjectionChangeRead::Changes {
                head,
                compacted_through: raced_watermark,
                changes,
            } => {
                assert_eq!(
                    head.as_ref().map(ProjectionChangeCursor::position),
                    Some(compacted_through + 3)
                );
                assert_eq!(raced_watermark, compacted_through);
                assert_eq!(
                    changes
                        .iter()
                        .map(|change| change.cursor.position())
                        .collect::<Vec<_>>(),
                    vec![compacted_through + 2, compacted_through + 3],
                    "repeatable snapshot returns the complete pre-compaction suffix"
                );
            }
            other => {
                panic!("PostgreSQL repeatable resume must return its complete page: {other:?}")
            }
        }
        assert!(matches!(
            repository
                .projection_changes(&topology(), &partition(), Some(&resume_after), 100)
                .await
                .unwrap(),
            ProjectionChangeRead::ResetRequired {
                compacted_through: fresh_watermark,
                ..
            } if fresh_watermark == compacted_through + 2
        ));
        assert!(matches!(
            repository
                .projection_changes(&topology(), &partition(), Some(&compact_through), 100)
                .await
                .unwrap(),
            ProjectionChangeRead::Changes {
                compacted_through: fresh_watermark,
                ref changes,
                ..
            } if fresh_watermark == compacted_through + 2
                && changes.len() == 1
                && changes[0].cursor.position() == compacted_through + 3
        ));
    }
}
