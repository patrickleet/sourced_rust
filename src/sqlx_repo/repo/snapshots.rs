use super::*;

impl<DB> SnapshotStore for SqlxRepository<DB>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    for<'c> &'c Pool<DB>: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> String: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> Vec<u8>: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
    for<'q> &'q [u8]: Encode<'q, DB> + Type<DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    fn get_snapshot<'a>(
        &'a self,
        identity: &'a StreamIdentity,
    ) -> impl Future<Output = Result<Option<SnapshotRecord>, RepositoryError>> + Send + 'a {
        async move {
            let mut builder = QueryBuilder::<DB>::new("SELECT ");
            builder.push(DB::SNAPSHOT_SELECT);
            builder.push(" FROM aggregate_snapshots WHERE aggregate_type = ");
            builder.push_bind(identity.aggregate_type());
            builder.push(" AND aggregate_id = ");
            builder.push_bind(identity.aggregate_id());
            let row = builder
                .build()
                .fetch_optional(&self.pool)
                .await
                .map_err(|err| repository_storage_error::<DB>("load snapshot", err))?;

            let Some(row) = row else {
                return Ok(None);
            };

            Ok(Some(snapshot_from_row::<DB>(row)?))
        }
    }

    fn get_snapshots<'a>(
        &'a self,
        identities: &'a [StreamIdentity],
    ) -> impl Future<Output = Result<Vec<SnapshotRecord>, RepositoryError>> + Send + 'a {
        async move {
            if identities.is_empty() {
                return Ok(Vec::new());
            }

            let mut records = Vec::with_capacity(identities.len());
            for (aggregate_type, aggregate_ids) in ids_by_type(identities) {
                let mut builder = QueryBuilder::<DB>::new("SELECT ");
                builder.push(DB::SNAPSHOT_SELECT);
                builder.push(" FROM aggregate_snapshots WHERE aggregate_type = ");
                builder.push_bind(aggregate_type);
                builder.push(" AND ");
                DB::push_id_filter(&mut builder, &aggregate_ids);
                let rows = builder
                    .build()
                    .fetch_all(&self.pool)
                    .await
                    .map_err(|err| repository_storage_error::<DB>("load snapshots", err))?;
                for row in rows {
                    records.push(snapshot_from_row::<DB>(row)?);
                }
            }
            Ok(records)
        }
    }

    fn save_snapshot<'a>(
        &'a self,
        identity: &'a StreamIdentity,
        record: SnapshotRecord,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a {
        async move {
            let mut tx =
                self.pool.begin().await.map_err(|err| {
                    repository_storage_error::<DB>("begin snapshot transaction", err)
                })?;
            save_snapshot_in_tx(&mut tx, identity, record).await?;
            tx.commit().await.map_err(|err| {
                repository_storage_error::<DB>("commit snapshot transaction", err)
            })?;
            Ok(())
        }
    }

    fn delete_snapshot<'a>(
        &'a self,
        identity: &'a StreamIdentity,
    ) -> impl Future<Output = Result<bool, RepositoryError>> + Send + 'a {
        async move {
            let mut builder =
                QueryBuilder::<DB>::new("DELETE FROM aggregate_snapshots WHERE aggregate_type = ");
            builder.push_bind(identity.aggregate_type());
            builder.push(" AND aggregate_id = ");
            builder.push_bind(identity.aggregate_id());
            let result = builder
                .build()
                .execute(&self.pool)
                .await
                .map_err(|err| repository_storage_error::<DB>("delete snapshot", err))?;

            Ok(DB::rows_affected(&result) > 0)
        }
    }
}
pub(super) async fn save_snapshot_in_tx<DB>(
    tx: &mut Transaction<'_, DB>,
    identity: &StreamIdentity,
    record: SnapshotRecord,
) -> Result<(), RepositoryError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
    for<'q> &'q [u8]: Encode<'q, DB> + Type<DB>,
{
    validate_snapshot_identity(identity, &record)?;

    let metadata = serialize_event_metadata(&record.metadata)?;
    let recorded_at = DB::timestamp_value(record.recorded_at)?;
    let version = repository_i64_from_u64(
        DB::BACKEND,
        record.version,
        "snapshot version",
        DB::INTEGER_STORAGE,
    )?;
    let snapshot_version = repository_i64_from_u64(
        DB::BACKEND,
        record.snapshot_version,
        "snapshot payload version",
        DB::INTEGER_STORAGE,
    )?;

    let mut builder = QueryBuilder::<DB>::new(
        "INSERT INTO aggregate_snapshots (\
         aggregate_type, aggregate_id, version, snapshot_version, payload, \
         payload_codec, payload_codec_version, metadata, recorded_at) VALUES (",
    );
    {
        let mut row = builder.separated(", ");
        row.push_bind(identity.aggregate_type())
            .push_bind(identity.aggregate_id())
            .push_bind(version)
            .push_bind(snapshot_version)
            .push_bind(record.payload.as_slice())
            .push_bind(record.payload_codec.as_str())
            .push_bind(i64::from(record.payload_codec_version));
        DB::push_metadata(&mut row, metadata.as_str());
        DB::push_timestamp(&mut row, &recorded_at);
    }
    builder.push(
        ") ON CONFLICT(aggregate_type, aggregate_id) DO UPDATE SET \
         version = excluded.version, \
         snapshot_version = excluded.snapshot_version, \
         payload = excluded.payload, \
         payload_codec = excluded.payload_codec, \
         payload_codec_version = excluded.payload_codec_version, \
         metadata = excluded.metadata, \
         recorded_at = excluded.recorded_at, \
         updated_at = ",
    );
    builder.push(DB::NOW);

    builder
        .build()
        .execute(&mut **tx)
        .await
        .map_err(|err| repository_storage_error::<DB>("save snapshot", err))?;

    Ok(())
}
pub(super) fn snapshot_from_row<DB>(row: DB::Row) -> Result<SnapshotRecord, RepositoryError>
where
    DB: SqlxRepoBackend,
    for<'q> i64: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> String: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> Vec<u8>: Type<DB> + sqlx::Decode<'q, DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    let metadata_json: String = row
        .try_get("metadata")
        .map_err(|err| repository_storage_error::<DB>("decode snapshot metadata row", err))?;
    Ok(SnapshotRecord {
        aggregate_type: row.try_get("aggregate_type").map_err(|err| {
            repository_storage_error::<DB>("decode snapshot aggregate type row", err)
        })?,
        aggregate_id: row.try_get("aggregate_id").map_err(|err| {
            repository_storage_error::<DB>("decode snapshot aggregate id row", err)
        })?,
        version: repository_u64_from_i64(
            DB::BACKEND,
            row.try_get("version").map_err(|err| {
                repository_storage_error::<DB>("decode snapshot version row", err)
            })?,
            "snapshot version",
        )?,
        snapshot_version: repository_u64_from_i64(
            DB::BACKEND,
            row.try_get("snapshot_version").map_err(|err| {
                repository_storage_error::<DB>("decode snapshot payload version row", err)
            })?,
            "snapshot payload version",
        )?,
        payload_codec: row.try_get("payload_codec").map_err(|err| {
            repository_storage_error::<DB>("decode snapshot payload codec row", err)
        })?,
        payload_codec_version: repository_u16_from_i64(
            DB::BACKEND,
            row.try_get("payload_codec_version").map_err(|err| {
                repository_storage_error::<DB>("decode snapshot payload codec version row", err)
            })?,
            "snapshot payload codec version",
        )?,
        payload: row
            .try_get("payload")
            .map_err(|err| repository_storage_error::<DB>("decode snapshot payload row", err))?,
        metadata: deserialize_event_metadata(&metadata_json)?,
        recorded_at: DB::decode_timestamp(&row, "recorded_at")?,
    })
}
