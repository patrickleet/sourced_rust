#![expect(
    clippy::manual_async_fn,
    reason = "async trait impls return impl Future + Send to preserve public Send bounds"
)]

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::{Arc, RwLock};

use crate::entity::{Entity, EventRecord};
use crate::outbox::OutboxMessage;
use crate::read_model::in_memory::apply_read_model_write_plan;
use crate::read_model::{
    InMemoryReadModelStore, ReadModelAdapterCapabilities, ReadModelCommitOutcome, ReadModelError,
    ReadModelLoadGraph, ReadModelLoadRequest, ReadModelQueryCapabilities, ReadModelWritePlan,
};
use crate::repository::{
    reject_duplicate_outbox_messages, reject_duplicate_streams,
    validate_entity_id_matches_identity, validate_prepared_appends, validate_snapshot_identity,
    CommitBatch, GetStream, InboxStore, PreparedEventAppend, ReadModelWritePlanStore,
    RelationalReadModelQueryStore, RepositoryError, SnapshotStore, SnapshotWrite, StreamIdentity,
    TransactionalCommit,
};
use crate::snapshot::{InMemorySnapshotStore, SnapshotRecord};

/// In-memory repository implementation using HashMap.
///
/// This repository is cheap to clone because it uses `Arc<RwLock<...>>`
/// internally - cloning creates another handle to the same storage.
/// Also includes an embedded `InMemoryReadModelStore` for read model storage.
///
/// Its consumer **inbox is dev-only**: the dedup set grows unbounded (no TTL,
/// no timestamps) and exists for tests and local development. Use the Postgres
/// or SQLite repository for a production inbox with retention control.
#[derive(Clone)]
pub struct HashMapRepository {
    event_store: Arc<RwLock<HashMap<String, Vec<EventRecord>>>>,
    outbox_store: Arc<RwLock<HashMap<String, OutboxMessage>>>,
    model_store: InMemoryReadModelStore,
    snapshot_store: InMemorySnapshotStore,
    /// Consumer inbox: the set of recorded `(consumer, message_id)` receipts.
    ///
    /// Dev-only: this set has no TTL and no timestamps, so it grows unbounded for
    /// the process lifetime and cannot be time-purged. It exists for tests and
    /// local development, not production effectively-once dedup — back a real
    /// deployment with the Postgres or SQLite inbox, which support
    /// [`purge_inbox_older_than`](crate::repository::InboxStore::purge_inbox_older_than).
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

    /// Drop every recorded inbox receipt, returning the count removed.
    ///
    /// The in-memory equivalent of inbox retention: because this dev-only inbox
    /// keeps no timestamps it cannot purge by age, so the only available control
    /// is to clear it wholesale (e.g. between test cases).
    pub fn clear_inbox(&self) -> usize {
        self.inbox_store
            .write()
            .map(|mut set| {
                let n = set.len();
                set.clear();
                n
            })
            .unwrap_or(0)
    }
}

impl GetStream for HashMapRepository {
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

impl TransactionalCommit for HashMapRepository {
    fn commit_batch<'a>(
        &'a self,
        batch: CommitBatch<'a>,
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
            for write in &batch.snapshots {
                validate_snapshot_write(write)?;
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
                    SnapshotWrite::Save { identity, record } => {
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

impl InboxStore for HashMapRepository {
    fn inbox_contains<'a>(
        &'a self,
        consumer: &'a str,
        message_id: &'a str,
    ) -> impl Future<Output = Result<bool, RepositoryError>> + Send + 'a {
        async move { Ok(self.inbox_contains(consumer, message_id)) }
    }

    fn purge_inbox_older_than(
        &self,
        _age: std::time::Duration,
    ) -> impl Future<Output = Result<u64, RepositoryError>> + Send {
        // The dev-only in-memory inbox keeps no timestamps, so age-based purge is
        // a no-op. Use `clear_inbox` to reset it wholesale.
        async move { Ok(0) }
    }
}

fn validate_snapshot_write(write: &SnapshotWrite) -> Result<(), RepositoryError> {
    match write {
        SnapshotWrite::Save { identity, record } => validate_snapshot_identity(identity, record),
    }
}

fn stored_stream_version(events: Option<&Vec<EventRecord>>) -> u64 {
    // A missing stream has committed version 0; the first appended event will
    // occupy sequence 1.
    events.map_or(0, |events| events.len() as u64)
}

impl ReadModelWritePlanStore for HashMapRepository {
    fn read_model_capabilities(&self) -> ReadModelAdapterCapabilities {
        self.model_store.read_model_capabilities()
    }

    fn commit_write_plan(
        &self,
        plan: ReadModelWritePlan,
    ) -> impl Future<Output = Result<ReadModelCommitOutcome, ReadModelError>> + Send + '_ {
        self.model_store.commit_write_plan(plan)
    }
}

impl RelationalReadModelQueryStore for HashMapRepository {
    fn read_model_query_capabilities(&self) -> ReadModelQueryCapabilities {
        self.model_store.read_model_query_capabilities()
    }

    fn load_graph(
        &self,
        request: ReadModelLoadRequest,
    ) -> impl Future<Output = Result<ReadModelLoadGraph, ReadModelError>> + Send + '_ {
        self.model_store.load_graph(request)
    }
}

impl SnapshotStore for HashMapRepository {
    fn get_snapshot<'a>(
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

    fn save_snapshot<'a>(
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

    fn delete_snapshot<'a>(
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
    use crate::repository::StreamWrite;

    fn identity(id: &str) -> StreamIdentity {
        StreamIdentity::new("test.aggregate", id).unwrap()
    }

    async fn commit_one(
        repo: &HashMapRepository,
        entity: &mut Entity,
    ) -> Result<(), RepositoryError> {
        let id = entity.id().to_string();
        repo.commit_batch(CommitBatch::new(vec![StreamWrite::new(
            identity(&id),
            entity,
        )]))
        .await
    }

    #[test]
    fn new() {
        let repo = HashMapRepository::new();
        assert!(repo.event_store.read().unwrap().is_empty());
    }

    #[tokio::test]
    async fn single_entity_commit() {
        let repo = HashMapRepository::new();
        let id = "test_id";
        let mut entity = Entity::with_id(id);

        entity.digest("test_event", &("arg1", "arg2")).unwrap();

        commit_one(&repo, &mut entity).await.unwrap();

        let fetched_entity = repo.get_stream(&identity(id)).await.unwrap().unwrap();
        assert_eq!(fetched_entity.id(), id);
        assert_eq!(fetched_entity.events(), entity.events());
    }

    #[tokio::test]
    async fn multiple_entity_commit() {
        let repo = HashMapRepository::new();

        let mut entity1 = Entity::with_id("id_1");
        entity1.digest("event1", &"arg1").unwrap();

        let mut entity2 = Entity::with_id("id_2");
        entity2.digest("event2", &"arg2").unwrap();

        repo.commit_batch(CommitBatch::new(vec![
            StreamWrite::new(identity("id_1"), &mut entity1),
            StreamWrite::new(identity("id_2"), &mut entity2),
        ]))
        .await
        .unwrap();

        let all_entities: Vec<Entity> = repo
            .get_streams(&[identity("id_1"), identity("id_2")])
            .await
            .unwrap();
        assert_eq!(all_entities.len(), 2);
    }

    #[tokio::test]
    async fn duplicate_stream_ids_rejected_before_write() {
        let repo = HashMapRepository::new();

        let mut entity1 = Entity::with_id("same-id");
        entity1.digest("event1", &"arg1").unwrap();

        let mut entity2 = Entity::with_id("same-id");
        entity2.digest("event2", &"arg2").unwrap();

        let err = repo
            .commit_batch(CommitBatch::new(vec![
                StreamWrite::new(identity("same-id"), &mut entity1),
                StreamWrite::new(identity("same-id"), &mut entity2),
            ]))
            .await
            .unwrap_err();
        assert!(
            matches!(&err, RepositoryError::DuplicateStreamInBatch { id } if *id == identity("same-id").to_string()),
            "unexpected error: {err}"
        );

        assert!(repo
            .get_stream(&identity("same-id"))
            .await
            .unwrap()
            .is_none());
        assert_eq!(entity1.committed_version(), 0);
        assert_eq!(entity2.committed_version(), 0);
        assert_eq!(entity1.new_events().len(), 1);
        assert_eq!(entity2.new_events().len(), 1);
    }

    #[tokio::test]
    async fn inbox_receipts_record_dedupe_and_roll_back_atomically() {
        use crate::repository::InboxReceipt;
        let repo = HashMapRepository::new();

        let mut batch = CommitBatch::empty();
        batch.inbox_receipts.push(InboxReceipt::new("proj", "m1"));
        repo.commit_batch(batch).await.unwrap();
        assert!(repo.inbox_contains("proj", "m1"));
        assert!(!repo.inbox_contains("proj", "m2"));

        // A batch with a duplicate (m1) and a fresh receipt (m2) rolls back whole.
        let mut dup = CommitBatch::empty();
        dup.inbox_receipts.push(InboxReceipt::new("proj", "m1"));
        dup.inbox_receipts.push(InboxReceipt::new("proj", "m2"));
        let err = repo.commit_batch(dup).await.unwrap_err();
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
            repo.commit_batch(invalid).await.unwrap_err(),
            RepositoryError::InvalidInboxReceipt { .. }
        ));
    }

    #[tokio::test]
    async fn clear_inbox_drops_all_receipts_and_age_purge_is_noop() {
        use crate::repository::{InboxReceipt, InboxStore};
        let repo = HashMapRepository::new();

        let mut batch = CommitBatch::empty();
        batch.inbox_receipts.push(InboxReceipt::new("proj", "m1"));
        batch.inbox_receipts.push(InboxReceipt::new("proj", "m2"));
        repo.commit_batch(batch).await.unwrap();

        // The dev-only inbox cannot purge by age (no timestamps): it is a no-op
        // that leaves the receipts in place.
        assert_eq!(
            repo.purge_inbox_older_than(std::time::Duration::from_secs(0))
                .await
                .unwrap(),
            0
        );
        assert!(repo.inbox_contains("proj", "m1"));

        // `clear_inbox` is the in-memory retention equivalent: it drops everything.
        assert_eq!(repo.clear_inbox(), 2);
        assert!(!repo.inbox_contains("proj", "m1"));
        assert!(!repo.inbox_contains("proj", "m2"));
        assert_eq!(repo.clear_inbox(), 0);
    }
}
