#![expect(
    clippy::manual_async_fn,
    reason = "async trait impls return impl Future + Send to preserve public Send bounds"
)]

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::{Arc, RwLock};

use crate::entity::{
    Committable, Entity, EventRecord, EventRecordError, BITCODE_PAYLOAD_CODEC,
    BITCODE_PAYLOAD_CODEC_VERSION,
};
use crate::outbox::OutboxMessage;
use crate::read_model::in_memory::apply_read_model_write_plan;
use crate::read_model::{
    InMemoryReadModelStore, ReadModelAdapterCapabilities, ReadModelCommitOutcome, ReadModelError,
    ReadModelLoadGraph, ReadModelLoadRequest, ReadModelQueryCapabilities, ReadModelWritePlan,
    ReadModelWritePlanStore, RelationalReadModelQueryStore,
};
use crate::repository::{
    AsyncCommitBatch, AsyncGetStream, AsyncInboxStore, AsyncReadModelWritePlanStore,
    AsyncRelationalReadModelQueryStore, AsyncSnapshotStore, AsyncSnapshotWrite, AsyncStreamWrite,
    AsyncTransactionalCommit, Commit, CommitBatch, GetMany, GetOne, PreparedEventAppend,
    RepositoryError, SnapshotWrite, StreamIdentity, TransactionalCommit,
};
use crate::snapshot::{InMemorySnapshotStore, SnapshotRecord, SnapshotStore};

/// In-memory repository implementation using HashMap.
///
/// This repository is cheap to clone because it uses `Arc<RwLock<...>>`
/// internally - cloning creates another handle to the same storage.
/// Also includes an embedded `InMemoryReadModelStore` for read model storage.
#[derive(Clone)]
pub struct HashMapRepository {
    event_store: Arc<RwLock<HashMap<String, Vec<EventRecord>>>>,
    outbox_store: Arc<RwLock<HashMap<String, OutboxMessage>>>,
    model_store: InMemoryReadModelStore,
    snapshot_store: InMemorySnapshotStore,
    /// Consumer inbox: the set of recorded `(consumer, message_id)` receipts.
    inbox_store: Arc<RwLock<HashSet<(String, String)>>>,
}

/// In-memory outbox table handle.
#[derive(Clone)]
pub struct HashMapOutboxStore {
    pub(crate) storage: Arc<RwLock<HashMap<String, OutboxMessage>>>,
}

impl Default for HashMapRepository {
    fn default() -> Self {
        Self::new()
    }
}

impl HashMapRepository {
    /// Create a new empty repository.
    pub fn new() -> Self {
        HashMapRepository {
            event_store: Arc::new(RwLock::new(HashMap::new())),
            outbox_store: Arc::new(RwLock::new(HashMap::new())),
            model_store: InMemoryReadModelStore::new(),
            snapshot_store: InMemorySnapshotStore::new(),
            inbox_store: Arc::new(RwLock::new(HashSet::new())),
        }
    }

    #[cfg(test)]
    pub(crate) fn outbox_storage(&self) -> &RwLock<HashMap<String, OutboxMessage>> {
        self.outbox_store.as_ref()
    }

    /// Access the in-memory outbox table handle.
    pub fn outbox_store(&self) -> HashMapOutboxStore {
        HashMapOutboxStore {
            storage: Arc::clone(&self.outbox_store),
        }
    }

    /// Access the embedded read model store directly.
    pub fn model_store(&self) -> &InMemoryReadModelStore {
        &self.model_store
    }

    /// Access the embedded snapshot store directly.
    pub fn snapshot_store(&self) -> &InMemorySnapshotStore {
        &self.snapshot_store
    }

    /// Whether a consumer inbox receipt for `(consumer, message_id)` is recorded.
    pub fn inbox_contains(&self, consumer: &str, message_id: &str) -> bool {
        self.inbox_store
            .read()
            .map(|set| set.contains(&(consumer.to_string(), message_id.to_string())))
            .unwrap_or(false)
    }
}

impl GetOne for HashMapRepository {
    fn get_one(&self, id: &str) -> Result<Option<Entity>, RepositoryError> {
        let storage = self
            .event_store
            .read()
            .map_err(|_| RepositoryError::LockPoisoned("read"))?;

        if let Some(events) = storage.get(id) {
            let mut entity = Entity::new();
            entity.set_id(id);
            entity.load_from_history(events.clone());
            Ok(Some(entity))
        } else {
            Ok(None)
        }
    }
}

impl GetMany for HashMapRepository {
    fn get_many(&self, ids: &[&str]) -> Result<Vec<Entity>, RepositoryError> {
        let mut entities = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(entity) = self.get_one(id)? {
                entities.push(entity);
            }
        }
        Ok(entities)
    }
}

impl AsyncGetStream for HashMapRepository {
    fn get_stream<'a>(
        &'a self,
        identity: &'a StreamIdentity,
    ) -> impl Future<Output = Result<Option<Entity>, RepositoryError>> + Send + 'a {
        async move {
            let storage = self
                .event_store
                .read()
                .map_err(|_| RepositoryError::LockPoisoned("async stream read"))?;

            if let Some(events) = storage.get(&identity.storage_key()) {
                let mut entity = Entity::new();
                entity.set_id(identity.aggregate_id());
                entity.load_from_history(events.clone());
                Ok(Some(entity))
            } else {
                Ok(None)
            }
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

impl Commit for HashMapRepository {
    fn commit<C: Committable + ?Sized>(&self, committable: &mut C) -> Result<(), RepositoryError> {
        let entities = committable.entities_mut();
        TransactionalCommit::commit_batch(self, CommitBatch::new(entities))
    }
}

impl AsyncTransactionalCommit for HashMapRepository {
    fn commit_batch_async<'a>(
        &'a self,
        batch: AsyncCommitBatch<'a>,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a {
        async move {
            reject_duplicate_async_streams(&batch.streams)?;
            validate_async_entity_id_matches_identity(&batch.streams)?;
            let prepared = batch
                .streams
                .iter()
                .map(PreparedEventAppend::from_stream_write)
                .collect::<Vec<_>>();
            validate_prepared_async_appends(&prepared)?;
            for write in &batch.snapshots {
                validate_async_snapshot_write(write)?;
            }
            reject_duplicate_outbox_messages(&batch.outbox_messages)?;

            let mut storage = self
                .event_store
                .write()
                .map_err(|_| RepositoryError::LockPoisoned("async stream write"))?;
            let mut relational_rows = self
                .model_store
                .relational_rows
                .write()
                .map_err(|_| RepositoryError::LockPoisoned("async read model write"))?;
            let mut snapshot_storage = self
                .snapshot_store
                .storage
                .write()
                .map_err(|_| RepositoryError::LockPoisoned("async snapshot write"))?;
            let mut outbox_storage = self
                .outbox_store
                .write()
                .map_err(|_| RepositoryError::LockPoisoned("async outbox write"))?;
            let mut inbox_storage = self
                .inbox_store
                .write()
                .map_err(|_| RepositoryError::LockPoisoned("async inbox write"))?;

            let mut staged_events = storage.clone();
            let mut staged_rows = relational_rows.clone();
            let mut staged_snapshots = snapshot_storage.clone();
            let mut staged_outbox = outbox_storage.clone();
            let mut staged_inbox = inbox_storage.clone();

            for append in &prepared {
                let stored_len =
                    stored_stream_version(staged_events.get(&append.identity.storage_key()));
                if stored_len != append.expected_version {
                    return Err(RepositoryError::ConcurrentWrite {
                        id: append.identity.to_string(),
                        expected: append.expected_version,
                        actual: stored_len,
                    });
                }
            }

            for append in prepared {
                let stored = staged_events
                    .entry(append.identity.storage_key())
                    .or_insert_with(Vec::new);
                stored.extend(append.events);
            }

            for plan in batch.read_model_plans {
                apply_read_model_write_plan(plan, &mut staged_rows)?;
            }

            for write in batch.snapshots {
                match write {
                    AsyncSnapshotWrite::Save { identity, record } => {
                        staged_snapshots.insert(identity.storage_key(), record);
                    }
                }
            }

            for message in batch.outbox_messages {
                let id = message.id().to_string();
                if staged_outbox.contains_key(&id) {
                    return Err(RepositoryError::DuplicateOutboxMessageInBatch { id });
                }
                staged_outbox.insert(id, message);
            }

            // Inbox receipts gate effectively-once: a receipt that already exists
            // (committed or duplicated in this batch) rolls the whole batch back so
            // effects are not double-applied.
            for receipt in batch.inbox_receipts {
                receipt.validate()?;
                let key = (receipt.consumer.clone(), receipt.message_id.clone());
                if !staged_inbox.insert(key) {
                    return Err(RepositoryError::DuplicateInboxReceipt {
                        consumer: receipt.consumer,
                        message_id: receipt.message_id,
                    });
                }
            }

            *storage = staged_events;
            *relational_rows = staged_rows;
            *snapshot_storage = staged_snapshots;
            *outbox_storage = staged_outbox;
            *inbox_storage = staged_inbox;

            for stream in batch.streams {
                stream.entity.mark_committed();
            }

            Ok(())
        }
    }
}

impl AsyncInboxStore for HashMapRepository {
    fn inbox_contains_async<'a>(
        &'a self,
        consumer: &'a str,
        message_id: &'a str,
    ) -> impl Future<Output = Result<bool, RepositoryError>> + Send + 'a {
        async move { Ok(self.inbox_contains(consumer, message_id)) }
    }
}

impl TransactionalCommit for HashMapRepository {
    fn commit_batch(&self, batch: CommitBatch<'_>) -> Result<(), RepositoryError> {
        reject_duplicate_streams(&batch.entities)?;
        reject_duplicate_outbox_messages(&batch.outbox_messages)?;

        let mut storage = self
            .event_store
            .write()
            .map_err(|_| RepositoryError::LockPoisoned("write"))?;
        let mut relational_rows = self
            .model_store
            .relational_rows
            .write()
            .map_err(|_| RepositoryError::LockPoisoned("read model write"))?;
        let mut snapshot_storage = self
            .snapshot_store
            .storage
            .write()
            .map_err(|_| RepositoryError::LockPoisoned("snapshot write"))?;
        let mut outbox_storage = self
            .outbox_store
            .write()
            .map_err(|_| RepositoryError::LockPoisoned("outbox write"))?;
        let mut inbox_storage = self
            .inbox_store
            .write()
            .map_err(|_| RepositoryError::LockPoisoned("inbox write"))?;

        let mut staged_events = storage.clone();
        let mut staged_rows = relational_rows.clone();
        let mut staged_snapshots = snapshot_storage.clone();
        let mut staged_outbox = outbox_storage.clone();
        let mut staged_inbox = inbox_storage.clone();

        // Phase 1: Validate all stream versions before staging any writes.
        for entity in &batch.entities {
            let stored_len = stored_stream_version(staged_events.get(entity.id()));
            if stored_len != entity.committed_version() {
                return Err(RepositoryError::ConcurrentWrite {
                    id: entity.id().to_string(),
                    expected: entity.committed_version(),
                    actual: stored_len,
                });
            }
        }

        // Phase 2: Apply every write to staged maps only.
        for entity in &batch.entities {
            let new_events = entity.new_events().to_vec();
            let stored = staged_events
                .entry(entity.id().to_string())
                .or_insert_with(Vec::new);
            stored.extend(new_events);
        }

        for plan in batch.read_model_plans {
            apply_read_model_write_plan(plan, &mut staged_rows)?;
        }

        for write in batch.snapshots {
            match write {
                SnapshotWrite::Save(record) => {
                    record.validate()?;
                    staged_snapshots.insert(record.aggregate_id.clone(), record);
                }
            }
        }

        for message in batch.outbox_messages {
            let id = message.id().to_string();
            if staged_outbox.contains_key(&id) {
                return Err(RepositoryError::DuplicateOutboxMessageInBatch { id });
            }
            staged_outbox.insert(id, message);
        }

        // Inbox receipts gate effectively-once (see the async impl).
        for receipt in batch.inbox_receipts {
            receipt.validate()?;
            let key = (receipt.consumer.clone(), receipt.message_id.clone());
            if !staged_inbox.insert(key) {
                return Err(RepositoryError::DuplicateInboxReceipt {
                    consumer: receipt.consumer,
                    message_id: receipt.message_id,
                });
            }
        }

        // Phase 3: Publish staged state only after all validation and staging succeeds.
        *storage = staged_events;
        *relational_rows = staged_rows;
        *snapshot_storage = staged_snapshots;
        *outbox_storage = staged_outbox;
        *inbox_storage = staged_inbox;

        for entity in batch.entities {
            entity.mark_committed();
        }

        Ok(())
    }
}

fn reject_duplicate_async_streams(streams: &[AsyncStreamWrite<'_>]) -> Result<(), RepositoryError> {
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

fn reject_duplicate_outbox_messages(messages: &[OutboxMessage]) -> Result<(), RepositoryError> {
    let mut seen = HashSet::with_capacity(messages.len());
    for message in messages {
        crate::outbox::validate_outbox_message_table_write(message)
            .map_err(|err| RepositoryError::Model(err.to_string()))?;
        let id = message.id();
        if id.trim().is_empty() {
            return Err(RepositoryError::Model(
                "outbox message id must not be empty".into(),
            ));
        }
        if message.event_type.trim().is_empty() {
            return Err(RepositoryError::Model(format!(
                "outbox message `{id}` event type must not be empty"
            )));
        }
        if !seen.insert(id.to_string()) {
            return Err(RepositoryError::DuplicateOutboxMessageInBatch { id: id.into() });
        }
    }
    Ok(())
}

fn validate_async_entity_id_matches_identity(
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

fn validate_prepared_async_appends(appends: &[PreparedEventAppend]) -> Result<(), RepositoryError> {
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

fn validate_async_snapshot_write(write: &AsyncSnapshotWrite) -> Result<(), RepositoryError> {
    match write {
        AsyncSnapshotWrite::Save { identity, record } => {
            validate_snapshot_identity(identity, record)
        }
    }
}

fn validate_snapshot_identity(
    identity: &StreamIdentity,
    record: &SnapshotRecord,
) -> Result<(), RepositoryError> {
    record.validate_for_identity(identity)
}

fn reject_duplicate_streams(entities: &[&mut Entity]) -> Result<(), RepositoryError> {
    let mut seen = HashSet::with_capacity(entities.len());
    for entity in entities {
        let id = entity.id();
        if !seen.insert(id.to_string()) {
            return Err(RepositoryError::DuplicateStreamInBatch { id: id.to_string() });
        }
    }
    Ok(())
}

fn stored_stream_version(events: Option<&Vec<EventRecord>>) -> u64 {
    // A missing stream has committed version 0; the first appended event will
    // occupy sequence 1.
    events.map_or(0, |events| events.len() as u64)
}

impl ReadModelWritePlanStore for HashMapRepository {
    fn read_model_capabilities(&self) -> ReadModelAdapterCapabilities {
        ReadModelWritePlanStore::read_model_capabilities(&self.model_store)
    }

    fn commit_write_plan(
        &self,
        plan: ReadModelWritePlan,
    ) -> Result<ReadModelCommitOutcome, ReadModelError> {
        ReadModelWritePlanStore::commit_write_plan(&self.model_store, plan)
    }
}

impl AsyncReadModelWritePlanStore for HashMapRepository {
    fn read_model_capabilities_async(&self) -> ReadModelAdapterCapabilities {
        ReadModelWritePlanStore::read_model_capabilities(self)
    }

    fn commit_write_plan_async(
        &self,
        plan: ReadModelWritePlan,
    ) -> impl Future<Output = Result<ReadModelCommitOutcome, ReadModelError>> + Send + '_ {
        async move { ReadModelWritePlanStore::commit_write_plan(self, plan) }
    }
}

impl RelationalReadModelQueryStore for HashMapRepository {
    fn read_model_query_capabilities(&self) -> ReadModelQueryCapabilities {
        RelationalReadModelQueryStore::read_model_query_capabilities(&self.model_store)
    }

    fn load_graph(
        &self,
        request: ReadModelLoadRequest,
    ) -> Result<ReadModelLoadGraph, ReadModelError> {
        RelationalReadModelQueryStore::load_graph(&self.model_store, request)
    }
}

impl AsyncRelationalReadModelQueryStore for HashMapRepository {
    fn read_model_query_capabilities_async(&self) -> ReadModelQueryCapabilities {
        RelationalReadModelQueryStore::read_model_query_capabilities(self)
    }

    fn load_graph_async(
        &self,
        request: ReadModelLoadRequest,
    ) -> impl Future<Output = Result<ReadModelLoadGraph, ReadModelError>> + Send + '_ {
        async move { RelationalReadModelQueryStore::load_graph(self, request) }
    }
}

impl SnapshotStore for HashMapRepository {
    fn get_snapshot(&self, id: &str) -> Result<Option<SnapshotRecord>, RepositoryError> {
        SnapshotStore::get_snapshot(&self.snapshot_store, id)
    }

    fn save_snapshot(&self, record: SnapshotRecord) -> Result<(), RepositoryError> {
        SnapshotStore::save_snapshot(&self.snapshot_store, record)
    }

    fn delete_snapshot(&self, id: &str) -> Result<bool, RepositoryError> {
        SnapshotStore::delete_snapshot(&self.snapshot_store, id)
    }
}

impl AsyncSnapshotStore for HashMapRepository {
    fn get_snapshot_async<'a>(
        &'a self,
        identity: &'a StreamIdentity,
    ) -> impl Future<Output = Result<Option<SnapshotRecord>, RepositoryError>> + Send + 'a {
        async move {
            let storage = self
                .snapshot_store
                .storage
                .read()
                .map_err(|_| RepositoryError::LockPoisoned("async snapshot read"))?;
            Ok(storage.get(&identity.storage_key()).cloned())
        }
    }

    fn save_snapshot_async<'a>(
        &'a self,
        identity: &'a StreamIdentity,
        record: SnapshotRecord,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a {
        async move {
            validate_snapshot_identity(identity, &record)?;
            let mut storage = self
                .snapshot_store
                .storage
                .write()
                .map_err(|_| RepositoryError::LockPoisoned("async snapshot write"))?;
            storage.insert(identity.storage_key(), record);
            Ok(())
        }
    }

    fn delete_snapshot_async<'a>(
        &'a self,
        identity: &'a StreamIdentity,
    ) -> impl Future<Output = Result<bool, RepositoryError>> + Send + 'a {
        async move {
            let mut storage = self
                .snapshot_store
                .storage
                .write()
                .map_err(|_| RepositoryError::LockPoisoned("async snapshot write"))?;
            Ok(storage.remove(&identity.storage_key()).is_some())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::Get;

    #[test]
    fn new() {
        let repo = HashMapRepository::new();
        assert!(repo.event_store.read().unwrap().is_empty());
    }

    #[test]
    fn single_entity_commit() {
        let repo = HashMapRepository::new();
        let id = "test_id";
        let mut entity = Entity::with_id(id);

        entity.digest("test_event", &("arg1", "arg2")).unwrap();

        repo.commit(&mut entity).unwrap();

        let fetched_entity = repo.get(id).unwrap().unwrap();
        assert_eq!(fetched_entity.id(), id);
        assert_eq!(fetched_entity.events(), entity.events());
    }

    #[test]
    fn multiple_entity_commit() {
        let repo = HashMapRepository::new();

        let mut entity1 = Entity::with_id("id_1");
        entity1.digest("event1", &"arg1").unwrap();

        let mut entity2 = Entity::with_id("id_2");
        entity2.digest("event2", &"arg2").unwrap();

        // Commit multiple entities using array syntax
        repo.commit(&mut [&mut entity1, &mut entity2]).unwrap();

        let all_entities: Vec<Entity> = repo.get(&["id_1", "id_2"]).unwrap();
        assert_eq!(all_entities.len(), 2);
    }

    #[test]
    fn duplicate_stream_ids_rejected_before_write() {
        let repo = HashMapRepository::new();

        let mut entity1 = Entity::with_id("same-id");
        entity1.digest("event1", &"arg1").unwrap();

        let mut entity2 = Entity::with_id("same-id");
        entity2.digest("event2", &"arg2").unwrap();

        let err = repo.commit(&mut [&mut entity1, &mut entity2]).unwrap_err();
        assert_eq!(
            err,
            RepositoryError::DuplicateStreamInBatch {
                id: "same-id".into()
            }
        );

        assert!(repo.get("same-id").unwrap().is_none());
        assert_eq!(entity1.committed_version(), 0);
        assert_eq!(entity2.committed_version(), 0);
        assert_eq!(entity1.new_events().len(), 1);
        assert_eq!(entity2.new_events().len(), 1);
    }

    #[test]
    fn inbox_receipts_record_dedupe_and_roll_back_atomically() {
        use crate::repository::InboxReceipt;
        let repo = HashMapRepository::new();

        let mut batch = CommitBatch::empty();
        batch.inbox_receipts.push(InboxReceipt::new("proj", "m1"));
        repo.commit_batch(batch).unwrap();
        assert!(repo.inbox_contains("proj", "m1"));
        assert!(!repo.inbox_contains("proj", "m2"));

        // A batch with a duplicate (m1) and a fresh receipt (m2) rolls back whole.
        let mut dup = CommitBatch::empty();
        dup.inbox_receipts.push(InboxReceipt::new("proj", "m1"));
        dup.inbox_receipts.push(InboxReceipt::new("proj", "m2"));
        let err = repo.commit_batch(dup).unwrap_err();
        assert!(
            matches!(err, RepositoryError::DuplicateInboxReceipt { ref message_id, .. } if message_id == "m1"),
            "got {err:?}"
        );
        assert!(
            !repo.inbox_contains("proj", "m2"),
            "the duplicate rolled the whole batch back"
        );

        // An empty receipt field is rejected (parity with the SQL CHECK).
        let mut invalid = CommitBatch::empty();
        invalid.inbox_receipts.push(InboxReceipt::new("", "m3"));
        assert!(matches!(
            repo.commit_batch(invalid).unwrap_err(),
            RepositoryError::InvalidInboxReceipt { .. }
        ));
    }
}
