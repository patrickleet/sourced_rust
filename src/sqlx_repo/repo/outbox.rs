use super::*;

/// One claimed-message lifecycle transition (the `UPDATE` shape is shared; only
/// the assignments differ).
enum OutboxTransition<'a> {
    Complete,
    Release { error: &'a str },
    Fail { error: &'a str },
}

impl OutboxTransition<'_> {
    fn target_status(&self) -> OutboxMessageStatus {
        match self {
            OutboxTransition::Complete => OutboxMessageStatus::Published,
            OutboxTransition::Release { .. } => OutboxMessageStatus::Pending,
            OutboxTransition::Fail { .. } => OutboxMessageStatus::Failed,
        }
    }

    fn operation(&self) -> &'static str {
        match self {
            OutboxTransition::Complete => "complete outbox message",
            OutboxTransition::Release { .. } => "release outbox message",
            OutboxTransition::Fail { .. } => "fail outbox message",
        }
    }
}

impl<DB> OutboxStore for SqlxOutboxStore<DB>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    for<'c> &'c Pool<DB>: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> f64: Encode<'q, DB> + Type<DB>,
    for<'q> String: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> Vec<u8>: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
    for<'q> &'q [u8]: Encode<'q, DB> + Type<DB>,
    for<'q> Option<i64>: Encode<'q, DB> + Type<DB>,
    for<'q> Option<&'q str>: Encode<'q, DB> + Type<DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    fn messages_by_status(
        &self,
        status: OutboxMessageStatus,
        limit: usize,
    ) -> impl Future<Output = Result<Vec<OutboxMessage>, RepositoryError>> + Send + '_ {
        async move {
            let mut builder = QueryBuilder::<DB>::new("SELECT ");
            builder.push(DB::OUTBOX_SELECT);
            builder.push(" FROM outbox_messages WHERE status = ");
            builder.push_bind(status.as_str());
            builder.push(" ORDER BY ");
            builder.push(DB::ORDER_BY_CREATED_AT);
            builder.push(" ASC, message_id ASC LIMIT ");
            // usize::MAX means "no practical bound"; clamp to what the column
            // type can carry.
            builder.push_bind(i64::try_from(limit).unwrap_or(i64::MAX));
            let rows = builder.build().fetch_all(&self.pool).await.map_err(|err| {
                repository_storage_error::<DB>("load outbox messages by status", err)
            })?;

            rows.into_iter()
                .map(outbox_message_from_row::<DB>)
                .collect()
        }
    }

    fn backlog_stats(
        &self,
    ) -> impl Future<Output = Result<OutboxBacklogStats, RepositoryError>> + Send + '_ {
        async move {
            let mut builder = QueryBuilder::<DB>::new("SELECT COUNT(*) AS pending_count, ");
            builder.push(DB::OUTBOX_OLDEST_CREATED_AT_SELECT);
            builder.push(" FROM outbox_messages WHERE status = ");
            builder.push_bind(OutboxMessageStatus::Pending.as_str());
            let row =
                builder.build().fetch_one(&self.pool).await.map_err(|err| {
                    repository_storage_error::<DB>("load outbox backlog stats", err)
                })?;

            let pending_count: i64 = row.try_get("pending_count").map_err(|err| {
                repository_storage_error::<DB>("decode outbox backlog count row", err)
            })?;
            let pending =
                repository_u64_from_i64(DB::BACKEND, pending_count, "outbox backlog count")
                    .and_then(|value| {
                        usize::try_from(value).map_err(|_| {
                            RepositoryError::Model(format!(
                                "{} outbox backlog count value {value} is invalid",
                                DB::BACKEND
                            ))
                        })
                    })?;
            let oldest_created_at = DB::decode_optional_timestamp(&row, "oldest_created_at")?;

            Ok(OutboxBacklogStats {
                pending,
                oldest_created_at,
            })
        }
    }

    fn claim<'a>(
        &'a self,
        request: ClaimOutboxMessages,
    ) -> impl Future<Output = Result<Vec<OutboxMessage>, RepositoryError>> + Send + 'a {
        DB::claim_outbox(&self.pool, request)
    }

    fn complete<'a>(
        &'a self,
        claim: &'a OutboxClaimRef,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a {
        transition_claimed_outbox_message(&self.pool, claim, OutboxTransition::Complete)
    }

    fn release<'a>(
        &'a self,
        claim: &'a OutboxClaimRef,
        error: &'a str,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a {
        transition_claimed_outbox_message(&self.pool, claim, OutboxTransition::Release { error })
    }

    fn fail<'a>(
        &'a self,
        claim: &'a OutboxClaimRef,
        error: &'a str,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a {
        transition_claimed_outbox_message(&self.pool, claim, OutboxTransition::Fail { error })
    }
}

/// Apply one claimed-message lifecycle transition (complete / release / fail).
///
/// The conditional `UPDATE` only applies while the caller still holds the
/// active claim (`status`, `claimed_by`, unexpired `claimed_until`, and
/// matching `attempts`); when no row is updated, the message is re-read to
/// produce the precise claim error.
async fn transition_claimed_outbox_message<'a, DB>(
    pool: &'a Pool<DB>,
    claim: &'a OutboxClaimRef,
    transition: OutboxTransition<'a>,
) -> Result<(), RepositoryError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c Pool<DB>: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> f64: Encode<'q, DB> + Type<DB>,
    for<'q> String: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> Vec<u8>: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
    for<'q> Option<i64>: Encode<'q, DB> + Type<DB>,
    for<'q> Option<&'q str>: Encode<'q, DB> + Type<DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    let now = SystemTime::now();
    let now_epoch = system_time_epoch_secs::<DB>(now)?;
    let now_value = DB::timestamp_value(now)?;

    let mut builder = QueryBuilder::<DB>::new("UPDATE outbox_messages SET status = ");
    builder.push_bind(transition.target_status().as_str());
    builder.push(", claimed_by = NULL, claimed_until = NULL, ");
    match &transition {
        OutboxTransition::Complete => {
            builder.push("published_at = ");
            DB::push_timestamp_assign(&mut builder, &now_value);
        }
        OutboxTransition::Release { error } => {
            builder.push("next_available_at = ");
            DB::push_timestamp_assign(&mut builder, &now_value);
            builder.push(", last_error = ");
            builder.push_bind(empty_string_as_none(error));
        }
        OutboxTransition::Fail { error } => {
            builder.push("last_error = ");
            builder.push_bind(empty_string_as_none(error));
            builder.push(", failed_at = ");
            DB::push_timestamp_assign(&mut builder, &now_value);
        }
    }
    builder.push(", updated_at = ");
    builder.push(DB::NOW);
    builder.push(" WHERE message_id = ");
    builder.push_bind(claim.message_id.as_str());
    builder.push(" AND status = ");
    builder.push_bind(OutboxMessageStatus::InFlight.as_str());
    builder.push(" AND claimed_by = ");
    builder.push_bind(claim.worker_id.as_str());
    builder.push(" AND claimed_until IS NOT NULL AND ");
    DB::push_timestamp_cmp(&mut builder, "claimed_until", ">", now_epoch);
    builder.push(" AND attempts = ");
    builder.push_bind(repository_i64_from_u64(
        DB::BACKEND,
        u64::from(claim.attempt),
        "outbox claim attempt",
        DB::INTEGER_STORAGE,
    )?);

    let result = builder
        .build()
        .execute(pool)
        .await
        .map_err(|err| repository_storage_error::<DB>(transition.operation(), err))?;

    ensure_outbox_update_applied(
        pool,
        DB::rows_affected(&result),
        &claim.message_id,
        |message| ensure_active_claim(message, Some(claim), now),
    )
    .await
}

/// Load an outbox message by id through any executor (pool or transaction).
pub(crate) async fn outbox_message_by_id<'e, DB, E>(
    executor: E,
    message_id: &str,
) -> Result<Option<OutboxMessage>, RepositoryError>
where
    DB: SqlxRepoBackend,
    E: Executor<'e, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> String: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> Vec<u8>: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    let mut builder = QueryBuilder::<DB>::new("SELECT ");
    builder.push(DB::OUTBOX_SELECT);
    builder.push(" FROM outbox_messages WHERE message_id = ");
    builder.push_bind(message_id);
    let row = builder
        .build()
        .fetch_optional(executor)
        .await
        .map_err(|err| repository_storage_error::<DB>("load outbox message", err))?;
    row.map(outbox_message_from_row::<DB>).transpose()
}

pub(crate) async fn ensure_outbox_update_applied<DB>(
    pool: &Pool<DB>,
    rows_affected: u64,
    message_id: &str,
    validate: impl FnOnce(&OutboxMessage) -> Result<(), RepositoryError>,
) -> Result<(), RepositoryError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c Pool<DB>: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> String: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> Vec<u8>: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    if rows_affected > 0 {
        return Ok(());
    }

    let message = outbox_message_by_id(pool, message_id)
        .await?
        .ok_or_else(|| RepositoryError::NotFound {
            id: message_id.to_string(),
        })?;
    validate(&message)
}
/// One `outbox_messages` row with pre-validated bind values.
struct OutboxRow<'a, DB: SqlxRepoBackend> {
    message_id: &'a str,
    event_type: &'a str,
    payload: &'a [u8],
    payload_codec: &'a str,
    payload_codec_version: i64,
    destination: Option<&'a str>,
    metadata: String,
    status: &'a str,
    created_at: DB::TimestampValue,
    worker_id: Option<&'a str>,
    leased_until: Option<DB::TimestampValue>,
    attempts: i64,
    last_error: Option<&'a str>,
    source_aggregate_type: Option<&'a str>,
    source_aggregate_id: Option<&'a str>,
    source_sequence: Option<i64>,
    correlation_id: Option<&'a str>,
    causation_id: Option<&'a str>,
}

/// Insert every outbox message with multi-row INSERTs (chunked to respect the
/// backend's bound-parameter limit). A unique violation on `message_id` still
/// maps to `DuplicateOutboxMessageInBatch`.
pub(super) async fn insert_outbox_messages_in_tx<DB>(
    tx: &mut Transaction<'_, DB>,
    messages: &[OutboxMessage],
) -> Result<(), RepositoryError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB>,
    for<'q> Option<i64>: Encode<'q, DB> + Type<DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
    for<'q> Option<&'q str>: Encode<'q, DB> + Type<DB>,
    for<'q> &'q [u8]: Encode<'q, DB> + Type<DB>,
{
    if messages.is_empty() {
        return Ok(());
    }

    let mut rows = Vec::with_capacity(messages.len());
    for message in messages {
        rows.push(OutboxRow::<DB> {
            message_id: message.id(),
            event_type: &message.event_type,
            payload: &message.payload,
            payload_codec: &message.payload_codec,
            payload_codec_version: i64::from(message.payload_codec_version),
            destination: message.destination.as_deref(),
            metadata: serialize_event_metadata(&message.metadata)?,
            status: message.status.as_str(),
            created_at: DB::timestamp_value(message.created_at)?,
            worker_id: message.worker_id.as_deref(),
            leased_until: message.leased_until.map(DB::timestamp_value).transpose()?,
            attempts: i64::from(message.attempts),
            last_error: message.last_error.as_deref(),
            source_aggregate_type: message.source_aggregate_type.as_deref(),
            source_aggregate_id: message.source_aggregate_id.as_deref(),
            source_sequence: message
                .source_sequence
                .map(|value| {
                    repository_i64_from_u64(
                        DB::BACKEND,
                        value,
                        "outbox source sequence",
                        DB::INTEGER_STORAGE,
                    )
                })
                .transpose()?,
            correlation_id: message.correlation_id(),
            causation_id: message.causation_id(),
        });
    }

    for chunk in rows.chunks(DB::MAX_BIND_PARAMS / OUTBOX_BIND_COLUMNS) {
        let mut builder = QueryBuilder::<DB>::new(
            "INSERT INTO outbox_messages (\
             message_id, event_type, payload, payload_codec, payload_codec_version, \
             destination, metadata, status, created_at, next_available_at, \
             claimed_by, claimed_until, attempts, last_error, source_aggregate_type, \
             source_aggregate_id, source_sequence, correlation_id, causation_id) ",
        );
        builder.push_values(chunk, |mut row, message| {
            row.push_bind(message.message_id)
                .push_bind(message.event_type)
                .push_bind(message.payload)
                .push_bind(message.payload_codec)
                .push_bind(message.payload_codec_version)
                .push_bind(message.destination);
            DB::push_metadata(&mut row, message.metadata.as_str());
            row.push_bind(message.status);
            // created_at and next_available_at share the same value.
            DB::push_timestamp(&mut row, &message.created_at);
            DB::push_timestamp(&mut row, &message.created_at);
            row.push_bind(message.worker_id);
            DB::push_optional_timestamp(&mut row, message.leased_until.as_ref());
            row.push_bind(message.attempts)
                .push_bind(message.last_error)
                .push_bind(message.source_aggregate_type)
                .push_bind(message.source_aggregate_id)
                .push_bind(message.source_sequence)
                .push_bind(message.correlation_id)
                .push_bind(message.causation_id);
        });

        let result = builder.build().execute(&mut **tx).await;
        if let Err(err) = result {
            if DB::is_unique_violation(&err) {
                // The batch was already deduped (validate_commit_batch), so a
                // violation means the id collides with a previously committed
                // row. Report the first id in the chunk, matching the per-row
                // path's contract.
                return Err(RepositoryError::DuplicateOutboxMessageInBatch {
                    id: chunk[0].message_id.to_string(),
                });
            }
            return Err(repository_storage_error::<DB>(
                "insert outbox messages",
                err,
            ));
        }
    }

    Ok(())
}
pub(crate) fn outbox_message_from_row<DB>(row: DB::Row) -> Result<OutboxMessage, RepositoryError>
where
    DB: SqlxRepoBackend,
    for<'q> i64: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> String: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> Vec<u8>: Type<DB> + sqlx::Decode<'q, DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    let status_text: String = row
        .try_get("status")
        .map_err(|err| repository_storage_error::<DB>("decode outbox status row", err))?;
    let status = status_text.parse::<OutboxMessageStatus>().map_err(|_| {
        RepositoryError::Model(format!(
            "{} outbox status `{status_text}` is invalid",
            DB::BACKEND
        ))
    })?;
    let metadata_json: String = row
        .try_get("metadata")
        .map_err(|err| repository_storage_error::<DB>("decode outbox metadata row", err))?;
    let attempts: i64 = row
        .try_get("attempts")
        .map_err(|err| repository_storage_error::<DB>("decode outbox attempts row", err))?;
    let source_sequence = row
        .try_get::<Option<i64>, _>("source_sequence")
        .map_err(|err| repository_storage_error::<DB>("decode outbox source sequence row", err))?
        .map(|value| repository_u64_from_i64(DB::BACKEND, value, "outbox source sequence"))
        .transpose()?;
    let mut metadata = deserialize_event_metadata(&metadata_json)?;
    if let Some(correlation_id) = row
        .try_get::<Option<String>, _>("correlation_id")
        .map_err(|err| repository_storage_error::<DB>("decode outbox correlation_id row", err))?
    {
        metadata.insert("correlation_id".into(), correlation_id);
    }
    if let Some(causation_id) = row
        .try_get::<Option<String>, _>("causation_id")
        .map_err(|err| repository_storage_error::<DB>("decode outbox causation_id row", err))?
    {
        metadata.insert("causation_id".into(), causation_id);
    }

    Ok(OutboxMessage {
        id: row
            .try_get("message_id")
            .map_err(|err| repository_storage_error::<DB>("decode outbox message id row", err))?,
        event_type: row
            .try_get("event_type")
            .map_err(|err| repository_storage_error::<DB>("decode outbox event type row", err))?,
        payload: row
            .try_get("payload")
            .map_err(|err| repository_storage_error::<DB>("decode outbox payload row", err))?,
        payload_codec: row.try_get("payload_codec").map_err(|err| {
            repository_storage_error::<DB>("decode outbox payload codec row", err)
        })?,
        payload_codec_version: repository_u16_from_i64(
            DB::BACKEND,
            row.try_get("payload_codec_version").map_err(|err| {
                repository_storage_error::<DB>("decode outbox payload codec version row", err)
            })?,
            "outbox payload codec version",
        )?,
        metadata,
        status,
        created_at: DB::decode_timestamp(&row, "created_at")?,
        worker_id: row
            .try_get("claimed_by")
            .map_err(|err| repository_storage_error::<DB>("decode outbox claimed_by row", err))?,
        leased_until: DB::decode_optional_timestamp(&row, "claimed_until")?,
        attempts: u32::try_from(attempts).map_err(|_| {
            RepositoryError::Model(format!(
                "{} outbox attempts value {attempts} is invalid",
                DB::BACKEND
            ))
        })?,
        last_error: row
            .try_get("last_error")
            .map_err(|err| repository_storage_error::<DB>("decode outbox last_error row", err))?,
        destination: row
            .try_get("destination")
            .map_err(|err| repository_storage_error::<DB>("decode outbox destination row", err))?,
        source_aggregate_type: row.try_get("source_aggregate_type").map_err(|err| {
            repository_storage_error::<DB>("decode outbox source aggregate type row", err)
        })?,
        source_aggregate_id: row.try_get("source_aggregate_id").map_err(|err| {
            repository_storage_error::<DB>("decode outbox source aggregate id row", err)
        })?,
        source_sequence,
    })
}
