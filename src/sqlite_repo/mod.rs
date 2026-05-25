//! SQLite-backed async repository and transactional document stores.
//!
//! This adapter is a local SQL persistence backend for the async repository
//! boundary. It is feature-gated behind `sqlite` and is intentionally async-only.

#![expect(
    clippy::manual_async_fn,
    reason = "async trait impls return impl Future + Send to preserve public Send bounds"
)]

use std::collections::HashSet;
use std::future::Future;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sqlx::sqlite::SqlitePoolOptions;
use sqlx::{Row, Sqlite, SqlitePool, Transaction};

use crate::entity::{
    Entity, EventRecord, EventRecordError, BITCODE_PAYLOAD_CODEC, BITCODE_PAYLOAD_CODEC_VERSION,
};
use crate::read_model::{
    ProcessedMessageMark, ReadModel, ReadModelAdapterCapabilities, ReadModelCommitOutcome,
    ReadModelError, ReadModelMutation, ReadModelWritePlan, Versioned,
};
use crate::repository::{
    AsyncCommitBatch, AsyncGetStream, AsyncReadModelSessionStore, AsyncReadModelStore,
    AsyncSnapshotStore, AsyncSnapshotWrite, AsyncStreamWrite, AsyncTransactionalCommit,
    PreparedEventAppend, RepositoryError, StreamIdentity,
};
use crate::snapshot::SnapshotRecord;

const SQLITE_SCHEMA: &str = include_str!("../../migrations/sqlite/0001_initial.sql");

/// SQLite-backed async repository.
#[derive(Clone)]
pub struct SqliteRepository {
    pool: SqlitePool,
}

impl SqliteRepository {
    /// Create a repository from an existing migrated pool.
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Open a SQLite pool without applying migrations.
    pub async fn connect(database_url: &str) -> Result<Self, RepositoryError> {
        let pool = SqlitePoolOptions::new()
            .max_connections(default_pool_size(database_url))
            .connect(database_url)
            .await
            .map_err(|err| repository_storage_error("connect", err))?;
        Ok(Self::new(pool))
    }

    /// Open a SQLite pool and apply the explicit SQLite migrations.
    pub async fn connect_and_migrate(database_url: &str) -> Result<Self, RepositoryError> {
        let repo = Self::connect(database_url).await?;
        repo.migrate().await?;
        Ok(repo)
    }

    /// Apply SQLite migrations to this repository's pool.
    pub async fn migrate(&self) -> Result<(), RepositoryError> {
        Self::migrate_pool(&self.pool).await
    }

    /// Apply SQLite migrations to an existing pool.
    pub async fn migrate_pool(pool: &SqlitePool) -> Result<(), RepositoryError> {
        for statement in SQLITE_SCHEMA.split(';') {
            let statement = statement.trim();
            if statement.is_empty() {
                continue;
            }
            sqlx::query(statement)
                .execute(pool)
                .await
                .map_err(|err| repository_storage_error("migrate", err))?;
        }
        Ok(())
    }

    /// Access the underlying SQLx pool for application-specific setup or tests.
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}

impl AsyncGetStream for SqliteRepository {
    fn get_stream<'a>(
        &'a self,
        identity: &'a StreamIdentity,
    ) -> impl Future<Output = Result<Option<Entity>, RepositoryError>> + Send + 'a {
        async move {
            let rows = sqlx::query(
                r#"
                SELECT event_name, event_version, payload, payload_codec,
                       payload_codec_version, metadata, sequence, recorded_at
                FROM aggregate_events
                WHERE aggregate_type = ? AND aggregate_id = ?
                ORDER BY sequence ASC
                "#,
            )
            .bind(identity.aggregate_type())
            .bind(identity.aggregate_id())
            .fetch_all(&self.pool)
            .await
            .map_err(|err| repository_storage_error("load stream", err))?;

            if rows.is_empty() {
                return Ok(None);
            }

            let mut events = Vec::with_capacity(rows.len());
            for row in rows {
                events.push(event_from_row(row)?);
            }

            let mut entity = Entity::new();
            entity.set_id(identity.aggregate_id());
            entity.load_from_history(events);
            Ok(Some(entity))
        }
    }

    fn get_streams<'a>(
        &'a self,
        identities: &'a [StreamIdentity],
    ) -> impl Future<Output = Result<Vec<Entity>, RepositoryError>> + Send + 'a {
        async move {
            let mut entities = Vec::with_capacity(identities.len());
            for identity in identities {
                if let Some(entity) = self.get_stream(identity).await? {
                    entities.push(entity);
                }
            }
            Ok(entities)
        }
    }
}

impl AsyncTransactionalCommit for SqliteRepository {
    fn commit_batch_async<'a>(
        &'a self,
        batch: AsyncCommitBatch<'a>,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a {
        async move {
            reject_duplicate_streams(&batch.streams)?;
            validate_entity_id_matches_identity(&batch.streams)?;

            let prepared = batch
                .streams
                .iter()
                .map(PreparedEventAppend::from_stream_write)
                .collect::<Vec<_>>();
            validate_prepared_appends(&prepared)?;

            for plan in &batch.read_model_plans {
                validate_document_write_plan(plan)?;
            }

            let mut tx = self
                .pool
                .begin()
                .await
                .map_err(|err| repository_storage_error("begin commit transaction", err))?;

            for append in &prepared {
                let actual = stream_version_in_tx(&mut tx, &append.identity).await?;
                if actual != append.expected_version {
                    return Err(RepositoryError::ConcurrentWrite {
                        id: append.identity.to_string(),
                        expected: append.expected_version,
                        actual,
                    });
                }
            }

            for append in &prepared {
                for event in &append.events {
                    insert_event_in_tx(&mut tx, &append.identity, append.expected_version, event)
                        .await?;
                }
            }

            for plan in batch.read_model_plans {
                let outcome = apply_document_write_plan_in_tx(&mut tx, plan).await?;
                if let Some(mark) = outcome.duplicate_message() {
                    return Err(RepositoryError::Model(format!(
                        "processed message already handled by consumer `{}`: `{}`",
                        mark.consumer_name, mark.message_id
                    )));
                }
            }

            for write in batch.snapshots {
                match write {
                    AsyncSnapshotWrite::Save { identity, record } => {
                        save_snapshot_in_tx(&mut tx, &identity, record).await?;
                    }
                }
            }

            tx.commit()
                .await
                .map_err(|err| repository_storage_error("commit transaction", err))?;

            for stream in batch.streams {
                stream.entity.mark_committed();
            }

            Ok(())
        }
    }
}

impl AsyncReadModelStore for SqliteRepository {
    fn get_model_async<'a, M>(
        &'a self,
        id: &'a str,
    ) -> impl Future<Output = Result<Option<Versioned<M>>, ReadModelError>> + Send + 'a
    where
        M: ReadModel + 'a,
    {
        async move { self.load_document_model::<M>(id).await }
    }

    fn get_by_primary_key_async<'a, M>(
        &'a self,
        id: &'a str,
    ) -> impl Future<Output = Result<Option<Versioned<M>>, ReadModelError>> + Send + 'a
    where
        M: ReadModel + 'a,
    {
        async move { self.load_document_model::<M>(id).await }
    }

    fn upsert_async<'a, M>(
        &'a self,
        model: &'a M,
    ) -> impl Future<Output = Result<Versioned<M>, ReadModelError>> + Send + 'a
    where
        M: ReadModel + 'a,
    {
        async move {
            let bytes =
                serde_json::to_vec(model).map_err(|err| ReadModelError::Serde(err.to_string()))?;
            let mut tx = begin_read_model_tx(&self.pool).await?;
            let version = upsert_document_in_tx(&mut tx, M::COLLECTION, model.id(), bytes).await?;
            commit_read_model_tx(tx).await?;
            Ok(Versioned {
                data: model.clone(),
                version,
            })
        }
    }

    fn insert_async<'a, M>(
        &'a self,
        model: &'a M,
    ) -> impl Future<Output = Result<Versioned<M>, ReadModelError>> + Send + 'a
    where
        M: ReadModel + 'a,
    {
        async move {
            let bytes =
                serde_json::to_vec(model).map_err(|err| ReadModelError::Serde(err.to_string()))?;
            let mut tx = begin_read_model_tx(&self.pool).await?;
            let existing = document_version_in_tx(&mut tx, M::COLLECTION, model.id()).await?;
            if let Some(actual) = existing {
                return Err(ReadModelError::ConcurrencyConflict {
                    collection: M::COLLECTION.to_string(),
                    id: model.id().to_string(),
                    expected: 0,
                    actual,
                });
            }
            insert_document_in_tx(&mut tx, M::COLLECTION, model.id(), bytes, 1).await?;
            commit_read_model_tx(tx).await?;
            Ok(Versioned {
                data: model.clone(),
                version: 1,
            })
        }
    }

    fn update_async<'a, M>(
        &'a self,
        model: &'a M,
        expected_version: u64,
    ) -> impl Future<Output = Result<Versioned<M>, ReadModelError>> + Send + 'a
    where
        M: ReadModel + 'a,
    {
        async move {
            let bytes =
                serde_json::to_vec(model).map_err(|err| ReadModelError::Serde(err.to_string()))?;
            let mut tx = begin_read_model_tx(&self.pool).await?;
            let actual = document_version_in_tx(&mut tx, M::COLLECTION, model.id())
                .await?
                .ok_or_else(|| ReadModelError::NotFound {
                    collection: M::COLLECTION.to_string(),
                    id: model.id().to_string(),
                })?;
            if actual != expected_version {
                return Err(ReadModelError::ConcurrencyConflict {
                    collection: M::COLLECTION.to_string(),
                    id: model.id().to_string(),
                    expected: expected_version,
                    actual,
                });
            }
            let new_version = next_document_version(M::COLLECTION, model.id(), Some(actual))?;
            update_document_in_tx(&mut tx, M::COLLECTION, model.id(), bytes, new_version).await?;
            commit_read_model_tx(tx).await?;
            Ok(Versioned {
                data: model.clone(),
                version: new_version,
            })
        }
    }

    fn delete_async<'a, M>(
        &'a self,
        id: &'a str,
    ) -> impl Future<Output = Result<bool, ReadModelError>> + Send + 'a
    where
        M: ReadModel + 'a,
    {
        async move {
            let result = sqlx::query(
                r#"
                DELETE FROM transactional_read_models
                WHERE collection = ? AND id = ?
                "#,
            )
            .bind(M::COLLECTION)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|err| read_model_storage_error("delete document", err))?;

            Ok(result.rows_affected() > 0)
        }
    }
}

impl SqliteRepository {
    async fn load_document_model<M: ReadModel>(
        &self,
        id: &str,
    ) -> Result<Option<Versioned<M>>, ReadModelError> {
        let row = sqlx::query(
            r#"
            SELECT payload, version
            FROM transactional_read_models
            WHERE collection = ? AND id = ?
            "#,
        )
        .bind(M::COLLECTION)
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|err| read_model_storage_error("load document", err))?;

        let Some(row) = row else {
            return Ok(None);
        };

        let payload: Vec<u8> = row
            .try_get("payload")
            .map_err(|err| read_model_storage_error("decode document payload row", err))?;
        let version = read_model_u64_from_i64(
            row.try_get("version")
                .map_err(|err| read_model_storage_error("decode document version row", err))?,
            "version",
        )?;
        let data = serde_json::from_slice(&payload)
            .map_err(|err| ReadModelError::Serde(err.to_string()))?;

        Ok(Some(Versioned { data, version }))
    }
}

impl AsyncReadModelSessionStore for SqliteRepository {
    fn read_model_capabilities_async(&self) -> ReadModelAdapterCapabilities {
        document_capabilities()
    }

    fn commit_write_plan_async(
        &self,
        plan: ReadModelWritePlan,
    ) -> impl Future<Output = Result<ReadModelCommitOutcome, ReadModelError>> + Send + '_ {
        async move {
            validate_document_write_plan(&plan)?;
            let mut tx = begin_read_model_tx(&self.pool).await?;
            let outcome = apply_document_write_plan_in_tx(&mut tx, plan).await?;
            if outcome.was_applied() {
                commit_read_model_tx(tx).await?;
            }
            Ok(outcome)
        }
    }

    fn is_processed_async<'a>(
        &'a self,
        consumer_name: &'a str,
        message_id: &'a str,
    ) -> impl Future<Output = Result<bool, ReadModelError>> + Send + 'a {
        async move { processed_message_exists_pool(&self.pool, consumer_name, message_id).await }
    }
}

impl AsyncSnapshotStore for SqliteRepository {
    fn get_snapshot_async<'a>(
        &'a self,
        identity: &'a StreamIdentity,
    ) -> impl Future<Output = Result<Option<SnapshotRecord>, RepositoryError>> + Send + 'a {
        async move {
            let row = sqlx::query(
                r#"
                SELECT aggregate_id, version, data
                FROM aggregate_snapshots
                WHERE aggregate_type = ? AND aggregate_id = ?
                "#,
            )
            .bind(identity.aggregate_type())
            .bind(identity.aggregate_id())
            .fetch_optional(&self.pool)
            .await
            .map_err(|err| repository_storage_error("load snapshot", err))?;

            let Some(row) = row else {
                return Ok(None);
            };

            Ok(Some(snapshot_from_row(row)?))
        }
    }

    fn save_snapshot_async<'a>(
        &'a self,
        identity: &'a StreamIdentity,
        record: SnapshotRecord,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a {
        async move {
            let mut tx = self
                .pool
                .begin()
                .await
                .map_err(|err| repository_storage_error("begin snapshot transaction", err))?;
            save_snapshot_in_tx(&mut tx, identity, record).await?;
            tx.commit()
                .await
                .map_err(|err| repository_storage_error("commit snapshot transaction", err))?;
            Ok(())
        }
    }

    fn delete_snapshot_async<'a>(
        &'a self,
        identity: &'a StreamIdentity,
    ) -> impl Future<Output = Result<bool, RepositoryError>> + Send + 'a {
        async move {
            let result = sqlx::query(
                r#"
                DELETE FROM aggregate_snapshots
                WHERE aggregate_type = ? AND aggregate_id = ?
                "#,
            )
            .bind(identity.aggregate_type())
            .bind(identity.aggregate_id())
            .execute(&self.pool)
            .await
            .map_err(|err| repository_storage_error("delete snapshot", err))?;

            Ok(result.rows_affected() > 0)
        }
    }
}

fn default_pool_size(database_url: &str) -> u32 {
    if database_url.contains(":memory:") {
        1
    } else {
        5
    }
}

fn reject_duplicate_streams(streams: &[AsyncStreamWrite<'_>]) -> Result<(), RepositoryError> {
    let mut seen = HashSet::with_capacity(streams.len());
    for stream in streams {
        let key = stream.identity.storage_key();
        if !seen.insert(key) {
            return Err(RepositoryError::DuplicateStreamInBatch {
                id: stream.identity.to_string(),
            });
        }
    }
    Ok(())
}

fn validate_entity_id_matches_identity(
    streams: &[AsyncStreamWrite<'_>],
) -> Result<(), RepositoryError> {
    for stream in streams {
        if stream.entity.id() != stream.identity.aggregate_id() {
            return Err(RepositoryError::Model(format!(
                "stream identity `{}` does not match entity id `{}`",
                stream.identity,
                stream.entity.id()
            )));
        }
    }
    Ok(())
}

fn validate_prepared_appends(appends: &[PreparedEventAppend]) -> Result<(), RepositoryError> {
    for append in appends {
        for (offset, event) in append.events.iter().enumerate() {
            validate_supported_event_codec(event)?;
            let expected_sequence = append.expected_version + offset as u64 + 1;
            if event.sequence != expected_sequence {
                return Err(RepositoryError::Model(format!(
                    "event `{}` for stream `{}` has sequence {}, expected {}",
                    event.event_name, append.identity, event.sequence, expected_sequence
                )));
            }
        }
    }
    Ok(())
}

fn validate_supported_event_codec(event: &EventRecord) -> Result<(), RepositoryError> {
    if event.payload_codec != BITCODE_PAYLOAD_CODEC
        || event.payload_codec_version != BITCODE_PAYLOAD_CODEC_VERSION
    {
        return Err(EventRecordError::unsupported_codec(
            &event.payload_codec,
            event.payload_codec_version,
        )
        .into());
    }
    Ok(())
}

async fn stream_version_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    identity: &StreamIdentity,
) -> Result<u64, RepositoryError> {
    let row = sqlx::query(
        r#"
        SELECT MAX(sequence) AS version
        FROM aggregate_events
        WHERE aggregate_type = ? AND aggregate_id = ?
        "#,
    )
    .bind(identity.aggregate_type())
    .bind(identity.aggregate_id())
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| repository_storage_error("load stream version", err))?;

    let version: Option<i64> = row
        .try_get("version")
        .map_err(|err| repository_storage_error("decode stream version row", err))?;
    version
        .map(|value| repository_u64_from_i64(value, "sequence"))
        .unwrap_or(Ok(0))
}

async fn insert_event_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    identity: &StreamIdentity,
    expected_version: u64,
    event: &EventRecord,
) -> Result<(), RepositoryError> {
    let metadata = serde_json::to_string(&event.metadata)
        .map_err(|err| RepositoryError::Model(format!("serialize event metadata: {err}")))?;

    let result = sqlx::query(
        r#"
        INSERT INTO aggregate_events (
          aggregate_type,
          aggregate_id,
          sequence,
          event_name,
          event_version,
          payload,
          payload_codec,
          payload_codec_version,
          metadata,
          recorded_at
        )
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(identity.aggregate_type())
    .bind(identity.aggregate_id())
    .bind(repository_i64_from_u64(event.sequence, "sequence")?)
    .bind(&event.event_name)
    .bind(repository_i64_from_u64(
        event.event_version,
        "event_version",
    )?)
    .bind(&event.payload)
    .bind(&event.payload_codec)
    .bind(i64::from(event.payload_codec_version))
    .bind(metadata)
    .bind(system_time_to_storage(event.timestamp)?)
    .execute(&mut **tx)
    .await;

    match result {
        Ok(_) => Ok(()),
        Err(err) if is_unique_constraint(&err) => {
            let actual = stream_version_in_tx(tx, identity).await?;
            Err(RepositoryError::ConcurrentWrite {
                id: identity.to_string(),
                expected: expected_version,
                actual,
            })
        }
        Err(err) => Err(repository_storage_error("insert event", err)),
    }
}

fn event_from_row(row: sqlx::sqlite::SqliteRow) -> Result<EventRecord, RepositoryError> {
    let payload_codec: String = row
        .try_get("payload_codec")
        .map_err(|err| repository_storage_error("decode payload codec row", err))?;
    let payload_codec_version = repository_u16_from_i64(
        row.try_get("payload_codec_version")
            .map_err(|err| repository_storage_error("decode payload codec version row", err))?,
        "payload_codec_version",
    )?;
    let metadata_json: String = row
        .try_get("metadata")
        .map_err(|err| repository_storage_error("decode metadata row", err))?;
    let metadata = serde_json::from_str(&metadata_json)
        .map_err(|err| RepositoryError::Model(format!("deserialize event metadata: {err}")))?;
    let event = EventRecord {
        event_name: row
            .try_get("event_name")
            .map_err(|err| repository_storage_error("decode event name row", err))?,
        payload_codec,
        payload_codec_version,
        payload: row
            .try_get("payload")
            .map_err(|err| repository_storage_error("decode payload row", err))?,
        event_version: repository_u64_from_i64(
            row.try_get("event_version")
                .map_err(|err| repository_storage_error("decode event version row", err))?,
            "event_version",
        )?,
        sequence: repository_u64_from_i64(
            row.try_get("sequence")
                .map_err(|err| repository_storage_error("decode sequence row", err))?,
            "sequence",
        )?,
        timestamp: system_time_from_storage(
            row.try_get::<String, _>("recorded_at")
                .map_err(|err| repository_storage_error("decode recorded_at row", err))?
                .as_str(),
        ),
        metadata,
    };
    validate_supported_event_codec(&event)?;
    Ok(event)
}

fn document_capabilities() -> ReadModelAdapterCapabilities {
    ReadModelAdapterCapabilities {
        relational_rows: false,
        document_rows: true,
        sparse_patches: false,
        deletes: false,
        processed_messages: true,
    }
}

fn validate_document_write_plan(plan: &ReadModelWritePlan) -> Result<(), ReadModelError> {
    for mutation in &plan.mutations {
        if !matches!(mutation, ReadModelMutation::Document(_)) {
            return Err(ReadModelError::Metadata(
                "SqliteRepository currently supports only document read-model mutations".into(),
            ));
        }
    }
    plan.validate_for(&document_capabilities())
}

async fn begin_read_model_tx(pool: &SqlitePool) -> Result<Transaction<'_, Sqlite>, ReadModelError> {
    pool.begin()
        .await
        .map_err(|err| read_model_storage_error("begin transaction", err))
}

async fn commit_read_model_tx(tx: Transaction<'_, Sqlite>) -> Result<(), ReadModelError> {
    tx.commit()
        .await
        .map_err(|err| read_model_storage_error("commit transaction", err))
}

async fn apply_document_write_plan_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    plan: ReadModelWritePlan,
) -> Result<ReadModelCommitOutcome, ReadModelError> {
    validate_document_write_plan(&plan)?;

    let mut marks_in_plan = HashSet::with_capacity(plan.processed_messages.len());
    for mark in &plan.processed_messages {
        let key = processed_message_key(mark);
        if !marks_in_plan.insert(key) || processed_message_exists_in_tx(tx, mark).await? {
            return Ok(ReadModelCommitOutcome::skipped_duplicate(mark.clone()));
        }
    }

    for mutation in plan.mutations {
        match mutation {
            ReadModelMutation::Document(mutation) => {
                upsert_document_in_tx(tx, &mutation.collection, &mutation.id, mutation.bytes)
                    .await?;
            }
            _ => {
                return Err(ReadModelError::Metadata(
                    "SqliteRepository currently supports only document read-model mutations".into(),
                ));
            }
        }
    }

    for mark in plan.processed_messages {
        let result = insert_processed_message_in_tx(tx, &mark).await;
        if let Err(err) = result {
            if is_unique_constraint(&err) {
                return Ok(ReadModelCommitOutcome::skipped_duplicate(mark));
            }
            return Err(read_model_storage_error("insert processed message", err));
        }
    }

    Ok(ReadModelCommitOutcome::applied())
}

async fn upsert_document_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    collection: &str,
    id: &str,
    bytes: Vec<u8>,
) -> Result<u64, ReadModelError> {
    let current = document_version_in_tx(tx, collection, id).await?;
    let new_version = next_document_version(collection, id, current)?;
    match current {
        Some(_) => update_document_in_tx(tx, collection, id, bytes, new_version).await?,
        None => insert_document_in_tx(tx, collection, id, bytes, new_version).await?,
    }
    Ok(new_version)
}

async fn document_version_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    collection: &str,
    id: &str,
) -> Result<Option<u64>, ReadModelError> {
    let row = sqlx::query(
        r#"
        SELECT version
        FROM transactional_read_models
        WHERE collection = ? AND id = ?
        "#,
    )
    .bind(collection)
    .bind(id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|err| read_model_storage_error("load document version", err))?;

    row.map(|row| {
        read_model_u64_from_i64(
            row.try_get("version")
                .map_err(|err| read_model_storage_error("decode document version row", err))?,
            "version",
        )
    })
    .transpose()
}

async fn insert_document_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    collection: &str,
    id: &str,
    bytes: Vec<u8>,
    version: u64,
) -> Result<(), ReadModelError> {
    sqlx::query(
        r#"
        INSERT INTO transactional_read_models (collection, id, version, payload)
        VALUES (?, ?, ?, ?)
        "#,
    )
    .bind(collection)
    .bind(id)
    .bind(read_model_i64_from_u64(version, "version")?)
    .bind(bytes)
    .execute(&mut **tx)
    .await
    .map_err(|err| read_model_storage_error("insert document", err))?;

    Ok(())
}

async fn update_document_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    collection: &str,
    id: &str,
    bytes: Vec<u8>,
    version: u64,
) -> Result<(), ReadModelError> {
    sqlx::query(
        r#"
        UPDATE transactional_read_models
        SET version = ?, payload = ?, updated_at = CURRENT_TIMESTAMP
        WHERE collection = ? AND id = ?
        "#,
    )
    .bind(read_model_i64_from_u64(version, "version")?)
    .bind(bytes)
    .bind(collection)
    .bind(id)
    .execute(&mut **tx)
    .await
    .map_err(|err| read_model_storage_error("update document", err))?;

    Ok(())
}

async fn processed_message_exists_pool(
    pool: &SqlitePool,
    consumer_name: &str,
    message_id: &str,
) -> Result<bool, ReadModelError> {
    let row = sqlx::query(
        r#"
        SELECT 1
        FROM read_model_processed_messages
        WHERE consumer_name = ? AND message_id = ?
        "#,
    )
    .bind(consumer_name)
    .bind(message_id)
    .fetch_optional(pool)
    .await
    .map_err(|err| read_model_storage_error("load processed message", err))?;
    Ok(row.is_some())
}

async fn processed_message_exists_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    mark: &ProcessedMessageMark,
) -> Result<bool, ReadModelError> {
    let row = sqlx::query(
        r#"
        SELECT 1
        FROM read_model_processed_messages
        WHERE consumer_name = ? AND message_id = ?
        "#,
    )
    .bind(&mark.consumer_name)
    .bind(&mark.message_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|err| read_model_storage_error("load processed message", err))?;
    Ok(row.is_some())
}

async fn insert_processed_message_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    mark: &ProcessedMessageMark,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO read_model_processed_messages (consumer_name, message_id)
        VALUES (?, ?)
        "#,
    )
    .bind(&mark.consumer_name)
    .bind(&mark.message_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn processed_message_key(mark: &ProcessedMessageMark) -> (String, String) {
    (mark.consumer_name.clone(), mark.message_id.clone())
}

async fn save_snapshot_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    identity: &StreamIdentity,
    record: SnapshotRecord,
) -> Result<(), RepositoryError> {
    if record.aggregate_id != identity.aggregate_id() {
        return Err(RepositoryError::Model(format!(
            "snapshot aggregate id `{}` does not match stream identity `{}`",
            record.aggregate_id, identity
        )));
    }

    sqlx::query(
        r#"
        INSERT INTO aggregate_snapshots (aggregate_type, aggregate_id, version, data)
        VALUES (?, ?, ?, ?)
        ON CONFLICT(aggregate_type, aggregate_id) DO UPDATE SET
          version = excluded.version,
          data = excluded.data,
          updated_at = CURRENT_TIMESTAMP
        "#,
    )
    .bind(identity.aggregate_type())
    .bind(identity.aggregate_id())
    .bind(repository_i64_from_u64(record.version, "snapshot version")?)
    .bind(record.data)
    .execute(&mut **tx)
    .await
    .map_err(|err| repository_storage_error("save snapshot", err))?;

    Ok(())
}

fn snapshot_from_row(row: sqlx::sqlite::SqliteRow) -> Result<SnapshotRecord, RepositoryError> {
    Ok(SnapshotRecord {
        aggregate_id: row
            .try_get("aggregate_id")
            .map_err(|err| repository_storage_error("decode snapshot aggregate id row", err))?,
        version: repository_u64_from_i64(
            row.try_get("version")
                .map_err(|err| repository_storage_error("decode snapshot version row", err))?,
            "snapshot version",
        )?,
        data: row
            .try_get("data")
            .map_err(|err| repository_storage_error("decode snapshot data row", err))?,
    })
}

fn next_document_version(
    collection: &str,
    id: &str,
    current_version: Option<u64>,
) -> Result<u64, ReadModelError> {
    match current_version {
        Some(version) => version.checked_add(1).ok_or_else(|| {
            ReadModelError::Storage(format!("read model version overflow for {collection}:{id}"))
        }),
        None => Ok(1),
    }
}

fn repository_i64_from_u64(value: u64, field: &str) -> Result<i64, RepositoryError> {
    i64::try_from(value).map_err(|_| {
        RepositoryError::Model(format!(
            "sqlite {field} value {value} exceeds signed integer storage"
        ))
    })
}

fn repository_u64_from_i64(value: i64, field: &str) -> Result<u64, RepositoryError> {
    u64::try_from(value)
        .map_err(|_| RepositoryError::Model(format!("sqlite {field} value {value} is negative")))
}

fn repository_u16_from_i64(value: i64, field: &str) -> Result<u16, RepositoryError> {
    u16::try_from(value)
        .map_err(|_| RepositoryError::Model(format!("sqlite {field} value {value} is invalid")))
}

fn read_model_i64_from_u64(value: u64, field: &str) -> Result<i64, ReadModelError> {
    i64::try_from(value).map_err(|_| {
        ReadModelError::Storage(format!(
            "sqlite {field} value {value} exceeds signed integer storage"
        ))
    })
}

fn read_model_u64_from_i64(value: i64, field: &str) -> Result<u64, ReadModelError> {
    u64::try_from(value)
        .map_err(|_| ReadModelError::Storage(format!("sqlite {field} value {value} is negative")))
}

fn system_time_to_storage(timestamp: SystemTime) -> Result<String, RepositoryError> {
    let duration = timestamp.duration_since(UNIX_EPOCH).map_err(|err| {
        RepositoryError::Model(format!(
            "event timestamp before UNIX epoch cannot be stored in sqlite: {err}"
        ))
    })?;
    Ok(format!(
        "{}.{:09}",
        duration.as_secs(),
        duration.subsec_nanos()
    ))
}

fn system_time_from_storage(value: &str) -> SystemTime {
    let Some((secs, nanos)) = value.split_once('.') else {
        return UNIX_EPOCH;
    };
    let Ok(secs) = secs.parse::<u64>() else {
        return UNIX_EPOCH;
    };
    let Ok(nanos) = nanos.parse::<u32>() else {
        return UNIX_EPOCH;
    };
    UNIX_EPOCH + Duration::new(secs, nanos)
}

fn is_unique_constraint(err: &sqlx::Error) -> bool {
    match err {
        sqlx::Error::Database(db_err) => {
            let message = db_err.message();
            let code = db_err.code().map(|code| code.into_owned());
            message.contains("UNIQUE constraint failed")
                || message.contains("PRIMARY KEY")
                || matches!(code.as_deref(), Some("1555" | "2067"))
        }
        _ => false,
    }
}

fn repository_storage_error(operation: &str, err: sqlx::Error) -> RepositoryError {
    RepositoryError::Model(format!("sqlite {operation} failed: {err}"))
}

fn read_model_storage_error(operation: &str, err: sqlx::Error) -> ReadModelError {
    ReadModelError::Storage(format!("sqlite {operation} failed: {err}"))
}
