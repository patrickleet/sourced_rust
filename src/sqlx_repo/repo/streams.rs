use super::*;

impl<DB> GetStream for SqlxRepository<DB>
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
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    fn get_stream<'a>(
        &'a self,
        identity: &'a StreamIdentity,
    ) -> impl Future<Output = Result<Option<Entity>, RepositoryError>> + Send + 'a {
        async move {
            let mut builder = QueryBuilder::<DB>::new("SELECT ");
            builder.push(DB::EVENT_SELECT);
            builder.push(" FROM aggregate_events WHERE aggregate_type = ");
            builder.push_bind(identity.aggregate_type());
            builder.push(" AND aggregate_id = ");
            builder.push_bind(identity.aggregate_id());
            builder.push(" ORDER BY sequence ASC");
            let rows = builder
                .build()
                .fetch_all(&self.pool)
                .await
                .map_err(|err| repository_storage_error::<DB>("load stream", err))?;

            if rows.is_empty() {
                return Ok(None);
            }

            let mut events = Vec::with_capacity(rows.len());
            for row in rows {
                events.push(event_from_row::<DB>(row)?);
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
            if identities.is_empty() {
                return Ok(Vec::new());
            }

            let mut entities = Vec::with_capacity(identities.len());
            for (aggregate_type, aggregate_ids) in ids_by_type(identities) {
                // Ordering by aggregate_id then sequence lets us slice the flat
                // result into per-aggregate entities in one pass. Callers of
                // `get_all` accept storage-order results.
                let mut builder = QueryBuilder::<DB>::new("SELECT aggregate_id, ");
                builder.push(DB::EVENT_SELECT);
                builder.push(" FROM aggregate_events WHERE aggregate_type = ");
                builder.push_bind(aggregate_type);
                builder.push(" AND ");
                DB::push_id_filter(&mut builder, &aggregate_ids);
                builder.push(" ORDER BY aggregate_id ASC, sequence ASC");

                let rows = builder
                    .build()
                    .fetch_all(&self.pool)
                    .await
                    .map_err(|err| repository_storage_error::<DB>("load streams", err))?;

                let mut current_id: Option<String> = None;
                let mut current_events: Vec<EventRecord> = Vec::new();
                for row in rows {
                    let row_id: String = row.try_get("aggregate_id").map_err(|err| {
                        repository_storage_error::<DB>("decode aggregate id row", err)
                    })?;
                    let event = event_from_row::<DB>(row)?;
                    match &current_id {
                        Some(id) if id == &row_id => current_events.push(event),
                        _ => {
                            if let Some(id) = current_id.take() {
                                entities.push(entity_from_events(
                                    id,
                                    std::mem::take(&mut current_events),
                                ));
                            }
                            current_id = Some(row_id);
                            current_events.push(event);
                        }
                    }
                }
                if let Some(id) = current_id.take() {
                    entities.push(entity_from_events(id, current_events));
                }
            }

            Ok(entities)
        }
    }

    fn get_stream_tail<'a>(
        &'a self,
        identity: &'a StreamIdentity,
        after_version: u64,
    ) -> impl Future<Output = Result<Option<Entity>, RepositoryError>> + Send + 'a {
        async move {
            // Fetch only the post-snapshot tail. `after_version` is the snapshot
            // version (an event sequence); `sequence > ?` skips already-folded
            // rows so a fresh snapshot over a long stream no longer reads and
            // decodes the entire history.
            let after = repository_i64_from_u64(
                DB::BACKEND,
                after_version,
                "snapshot tail lower bound",
                DB::INTEGER_STORAGE,
            )?;
            let mut builder = QueryBuilder::<DB>::new("SELECT ");
            builder.push(DB::EVENT_SELECT);
            builder.push(" FROM aggregate_events WHERE aggregate_type = ");
            builder.push_bind(identity.aggregate_type());
            builder.push(" AND aggregate_id = ");
            builder.push_bind(identity.aggregate_id());
            builder.push(" AND sequence > ");
            builder.push_bind(after);
            builder.push(" ORDER BY sequence ASC");
            let rows = builder
                .build()
                .fetch_all(&self.pool)
                .await
                .map_err(|err| repository_storage_error::<DB>("load stream tail", err))?;

            let mut events = Vec::with_capacity(rows.len());
            for row in rows {
                events.push(event_from_row::<DB>(row)?);
            }

            // Empty tail is "snapshot current", "snapshot ahead of the stream",
            // or "no event rows" (sqlite hardening deletes pre-snapshot rows).
            // MAX(sequence) distinguishes a planted future snapshot (clamp) from
            // a snapshot-only load (no rows → keep after_version).
            let prefix = if events.is_empty() {
                let stream_version =
                    super::commit::stream_version::<DB, _>(&self.pool, identity).await?;
                if stream_version == 0 {
                    after_version
                } else {
                    after_version.min(stream_version)
                }
            } else {
                after_version
            };

            let mut entity = Entity::new();
            entity.set_id(identity.aggregate_id());
            entity.load_tail_from_history(events, prefix);
            Ok(Some(entity))
        }
    }
}

impl<DB> CausalGetStream for SqlxRepository<DB>
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
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    fn get_causal_stream<'a>(
        &'a self,
        identity: &'a StreamIdentity,
    ) -> impl Future<Output = Result<Option<Entity>, RepositoryError>> + Send + 'a {
        GetStream::get_stream(self, identity)
    }
}

impl<DB> CausalRepositoryIdentity for SqlxRepository<DB>
where
    DB: SqlxRepoBackend,
{
    fn causal_storage_identity(&self) -> CausalStorageIdentity {
        self.causal_storage_identity
    }
}
pub(super) fn entity_from_events(aggregate_id: String, events: Vec<EventRecord>) -> Entity {
    let mut entity = Entity::new();
    entity.set_id(aggregate_id);
    entity.load_from_history(events);
    entity
}
