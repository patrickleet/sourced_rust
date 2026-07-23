#![expect(
    clippy::manual_async_fn,
    reason = "async trait impls return impl Future + Send to preserve public Send bounds"
)]

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::{Arc, RwLock};
use std::time::SystemTime;

use crate::command_ledger::{
    AttemptFence, CausalCommitBatch, CausalGetStream, CausalRepositoryIdentity,
    CausalStorageIdentity, CausalTransactionalCommit, CommandCompletion, CommandLedgerError,
    CommandLedgerKey, CommandLedgerRecord, CommandLedgerStore, CommandLookup, CommandLookupScope,
    CommandReservation, ReservationDecision, ReservationOutcome,
};
use crate::entity::{Entity, EventRecord};
use crate::outbox::OutboxMessage;
use crate::read_model::in_memory::apply_read_model_write_plan;
use crate::read_model::{
    InMemoryReadModelStore, ReadModelLoadGraph, ReadModelLoadRequest, ReadModelQueryCapabilities,
};
use crate::repository::{
    validate_commit_batch, validate_snapshot_identity, CommitBatch, GetStream, InboxStore,
    ReadModelWritePlanStore, RelationalReadModelQueryStore, RepositoryError, SnapshotStore,
    SnapshotWrite, StreamIdentity, TransactionalCommit,
};
use crate::snapshot::{InMemorySnapshotStore, SnapshotRecord};
use crate::table::{TableAdapterCapabilities, TableCommitOutcome, TableStoreError, TableWritePlan};

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
pub struct InMemoryRepository {
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
    command_ledger: Arc<RwLock<HashMap<CommandLedgerKey, CommandLedgerRecord>>>,
    #[cfg_attr(not(feature = "graphql"), allow(dead_code))]
    causal_storage_identity: CausalStorageIdentity,
}

/// In-memory outbox table handle.
#[derive(Clone)]
pub struct InMemoryOutboxStore {
    pub(crate) storage: Arc<RwLock<HashMap<String, OutboxMessage>>>,
}

impl Default for InMemoryRepository {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryRepository {
    /// Create a new empty repository.
    pub fn new() -> Self {
        InMemoryRepository {
            event_store: Arc::new(RwLock::new(HashMap::new())),
            outbox_store: Arc::new(RwLock::new(HashMap::new())),
            model_store: InMemoryReadModelStore::new(),
            snapshot_store: InMemorySnapshotStore::new(),
            inbox_store: Arc::new(RwLock::new(HashSet::new())),
            command_ledger: Arc::new(RwLock::new(HashMap::new())),
            causal_storage_identity: CausalStorageIdentity::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn causal_storage_identity(&self) -> CausalStorageIdentity {
        self.causal_storage_identity
    }

    #[cfg(test)]
    pub(crate) fn outbox_storage(&self) -> &RwLock<HashMap<String, OutboxMessage>> {
        self.outbox_store.as_ref()
    }

    /// Access the in-memory outbox table handle.
    pub fn outbox_store(&self) -> InMemoryOutboxStore {
        InMemoryOutboxStore {
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

enum InMemoryCommitError {
    Repository(RepositoryError),
    Ledger(CommandLedgerError),
}

impl From<RepositoryError> for InMemoryCommitError {
    fn from(error: RepositoryError) -> Self {
        Self::Repository(error)
    }
}

impl From<CommandLedgerError> for InMemoryCommitError {
    fn from(error: CommandLedgerError) -> Self {
        Self::Ledger(error)
    }
}

impl From<TableStoreError> for InMemoryCommitError {
    fn from(error: TableStoreError) -> Self {
        Self::Repository(RepositoryError::from(error))
    }
}

impl InMemoryRepository {
    fn commit_batch_inner<'a>(
        &'a self,
        batch: CommitBatch<'a>,
        completion: Option<CommandCompletion>,
    ) -> Result<(), InMemoryCommitError> {
        let prepared = validate_commit_batch(&batch)?;

        // All-or-nothing without cloning whole stores: every fallible check
        // below runs against live maps or a batch-bounded staging copy. The
        // ledger guard is deliberately acquired last to preserve one global
        // lock order, then its exact attempt fence is validated before any
        // durable map is mutated.
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
        let mut ledger_storage = completion
            .as_ref()
            .map(|_| {
                self.command_ledger
                    .write()
                    .map_err(|_| RepositoryError::LockPoisoned("command ledger write"))
            })
            .transpose()?;

        // Events: optimistic-concurrency check (reads only; appends cannot
        // fail once every stream passed).
        for append in &prepared {
            let stored_len = stored_stream_version(storage.get(&append.identity.storage_key()));
            if stored_len != append.expected_version {
                return Err(RepositoryError::ConcurrentWrite {
                    id: append.identity.to_string(),
                    expected: append.expected_version,
                    actual: stored_len,
                }
                .into());
            }
        }

        // Read models can fail mid-application, so stage only touched rows.
        let touched_rows: HashSet<String> = batch
            .read_model_plans
            .iter()
            .flat_map(|plan| plan.mutations.iter().map(|mutation| mutation.lock_key()))
            .collect();
        let mut staged_rows = HashMap::with_capacity(touched_rows.len());
        for key in &touched_rows {
            if let Some(row) = relational_rows.get(key) {
                staged_rows.insert(key.clone(), row.clone());
            }
        }
        for plan in batch.read_model_plans.iter().cloned() {
            apply_read_model_write_plan(plan, &mut staged_rows)?;
        }
        debug_assert!(
            staged_rows.keys().all(|key| touched_rows.contains(key)),
            "read model plan wrote a row outside its mutations' lock keys"
        );

        for message in &batch.outbox_messages {
            if outbox_storage.contains_key(message.id()) {
                return Err(RepositoryError::DuplicateOutboxMessageInBatch {
                    id: message.id().to_string(),
                }
                .into());
            }
        }

        let mut batch_receipts = HashSet::with_capacity(batch.inbox_receipts.len());
        for receipt in &batch.inbox_receipts {
            receipt.validate()?;
            let key = (receipt.consumer.as_str(), receipt.message_id.as_str());
            if inbox_storage.contains(&(receipt.consumer.clone(), receipt.message_id.clone()))
                || !batch_receipts.insert(key)
            {
                return Err(RepositoryError::DuplicateInboxReceipt {
                    consumer: receipt.consumer.clone(),
                    message_id: receipt.message_id.clone(),
                }
                .into());
            }
        }

        // Stage the terminal row only after every other fallible validation so
        // its live-lease check is as close as possible to the atomic mutation
        // boundary. The staged row remains hidden; assigning it to the held
        // ledger map is the final mutation below.
        let staged_ledger_record = completion
            .as_ref()
            .map(|completion| {
                let record = ledger_storage
                    .as_ref()
                    .and_then(|ledger| ledger.get(completion.attempt().key()))
                    .ok_or_else(|| CommandLedgerError::AttemptFenced {
                        command_id: completion.attempt().key().command_id().to_string(),
                    })?;
                let mut staged = record.clone();
                staged.complete(completion, SystemTime::now())?;
                Ok::<_, CommandLedgerError>(staged)
            })
            .transpose()?;

        // Nothing below can fail: mutate the live stores.
        for append in prepared {
            storage
                .entry(append.identity.storage_key())
                .or_insert_with(Vec::new)
                .extend_from_slice(append.events);
        }
        for key in touched_rows {
            match staged_rows.remove(&key) {
                Some(row) => {
                    relational_rows.insert(key, row);
                }
                None => {
                    relational_rows.remove(&key);
                }
            }
        }
        for write in batch.snapshots {
            match write {
                SnapshotWrite::Save { identity, record } => {
                    snapshot_storage.insert(identity.storage_key(), record);
                }
            }
        }
        for message in batch.outbox_messages {
            outbox_storage.insert(message.id().to_string(), message);
        }
        for receipt in batch.inbox_receipts {
            inbox_storage.insert((receipt.consumer, receipt.message_id));
        }
        for stream in batch.streams {
            stream.entity.mark_committed();
        }

        if let (Some(ledger), Some(record)) = (ledger_storage.as_mut(), staged_ledger_record) {
            ledger.insert(record.key.clone(), record);
        }
        Ok(())
    }
}

impl GetStream for InMemoryRepository {
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
}

impl CausalGetStream for InMemoryRepository {
    fn get_causal_stream<'a>(
        &'a self,
        identity: &'a StreamIdentity,
    ) -> impl Future<Output = Result<Option<Entity>, RepositoryError>> + Send + 'a {
        GetStream::get_stream(self, identity)
    }
}

impl CausalRepositoryIdentity for InMemoryRepository {
    fn causal_storage_identity(&self) -> CausalStorageIdentity {
        self.causal_storage_identity
    }
}

impl CommandLedgerStore for InMemoryRepository {
    fn reserve_command(
        &self,
        reservation: CommandReservation,
    ) -> impl Future<Output = Result<ReservationOutcome, CommandLedgerError>> + Send + '_ {
        async move {
            let now = SystemTime::now();
            let mut ledger = self
                .command_ledger
                .write()
                .map_err(|_| RepositoryError::LockPoisoned("command ledger reserve"))?;

            match ledger.entry(reservation.key().clone()) {
                std::collections::hash_map::Entry::Vacant(entry) => {
                    let record = CommandLedgerRecord::initial(&reservation, now)?;
                    let outcome = ReservationOutcome::Acquired(record.acquired_attempt()?);
                    entry.insert(record);
                    Ok(outcome)
                }
                std::collections::hash_map::Entry::Occupied(mut entry) => {
                    let decision = entry.get().classify_reservation(&reservation, now)?;
                    match decision {
                        ReservationDecision::Expire => {
                            entry.get_mut().expire(now);
                            Ok(ReservationOutcome::Expired)
                        }
                        ReservationDecision::Reclaim => {
                            entry.get_mut().reclaim(&reservation, now)?;
                            entry.get().reservation_outcome(decision)
                        }
                        other => entry.get().reservation_outcome(other),
                    }
                }
            }
        }
    }

    fn lookup_command<'a>(
        &'a self,
        key: &'a CommandLedgerKey,
        scope: CommandLookupScope<'a>,
    ) -> impl Future<Output = Result<CommandLookup, CommandLedgerError>> + Send + 'a {
        async move {
            let now = SystemTime::now();
            let mut ledger = self
                .command_ledger
                .write()
                .map_err(|_| RepositoryError::LockPoisoned("command ledger lookup"))?;
            let Some(record) = ledger.get_mut(key) else {
                return Ok(CommandLookup::Unknown);
            };
            if !record.matches_lookup_scope(scope) {
                return Ok(CommandLookup::Unknown);
            }
            if record.state != crate::command_ledger::CommandLedgerState::Expired
                && record.retention_expires_at <= now
            {
                record.expire(now);
            }
            record.lookup()
        }
    }

    fn mark_retryable_unknown(
        &self,
        attempt: AttemptFence,
    ) -> impl Future<Output = Result<(), CommandLedgerError>> + Send + '_ {
        async move {
            let mut ledger = self
                .command_ledger
                .write()
                .map_err(|_| RepositoryError::LockPoisoned("command ledger retryable update"))?;
            let record =
                ledger
                    .get_mut(attempt.key())
                    .ok_or_else(|| CommandLedgerError::AttemptFenced {
                        command_id: attempt.key().command_id().to_string(),
                    })?;
            record.mark_retryable_unknown(&attempt, SystemTime::now())
        }
    }

    fn compact_expired_commands(
        &self,
        limit: usize,
    ) -> impl Future<Output = Result<u64, CommandLedgerError>> + Send + '_ {
        async move {
            if limit == 0 {
                return Ok(0);
            }
            let now = SystemTime::now();
            let mut ledger = self
                .command_ledger
                .write()
                .map_err(|_| RepositoryError::LockPoisoned("command ledger compaction"))?;
            let mut candidates = ledger
                .iter()
                .filter(|(_, record)| {
                    record.state != crate::command_ledger::CommandLedgerState::Expired
                        && record.retention_expires_at <= now
                })
                .map(|(key, record)| (record.retention_expires_at, key.clone()))
                .collect::<Vec<_>>();
            candidates.sort_by(|left, right| {
                left.0.cmp(&right.0).then_with(|| {
                    left.1
                        .service_id()
                        .cmp(right.1.service_id())
                        .then_with(|| {
                            left.1
                                .principal_partition()
                                .cmp(right.1.principal_partition())
                        })
                        .then_with(|| left.1.command_id().cmp(right.1.command_id()))
                })
            });

            let mut compacted = 0;
            for (_, key) in candidates.into_iter().take(limit) {
                if let Some(record) = ledger.get_mut(&key) {
                    record.expire(now);
                    compacted += 1;
                }
            }
            Ok(compacted)
        }
    }
}

impl TransactionalCommit for InMemoryRepository {
    fn commit_batch<'a>(
        &'a self,
        batch: CommitBatch<'a>,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a {
        async move {
            match self.commit_batch_inner(batch, None) {
                Ok(()) => Ok(()),
                Err(InMemoryCommitError::Repository(error)) => Err(error),
                Err(InMemoryCommitError::Ledger(error)) => Err(RepositoryError::Model(format!(
                    "unexpected command ledger error in ordinary commit: {error}"
                ))),
            }
        }
    }
}

impl CausalTransactionalCommit for InMemoryRepository {
    fn commit_causal_batch<'a>(
        &'a self,
        batch: CausalCommitBatch<'a>,
    ) -> impl Future<Output = Result<(), CommandLedgerError>> + Send + 'a {
        async move {
            match self.commit_batch_inner(batch.domain, Some(batch.completion)) {
                Ok(()) => Ok(()),
                Err(InMemoryCommitError::Repository(error)) => {
                    Err(CommandLedgerError::Storage(error))
                }
                Err(InMemoryCommitError::Ledger(error)) => Err(error),
            }
        }
    }
}

impl InboxStore for InMemoryRepository {
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

fn stored_stream_version(events: Option<&Vec<EventRecord>>) -> u64 {
    // A missing stream has committed version 0; the first appended event will
    // occupy sequence 1.
    events.map_or(0, |events| events.len() as u64)
}

impl ReadModelWritePlanStore for InMemoryRepository {
    fn read_model_capabilities(&self) -> TableAdapterCapabilities {
        self.model_store.read_model_capabilities()
    }

    fn commit_write_plan(
        &self,
        plan: TableWritePlan,
    ) -> impl Future<Output = Result<TableCommitOutcome, TableStoreError>> + Send + '_ {
        self.model_store.commit_write_plan(plan)
    }
}

impl RelationalReadModelQueryStore for InMemoryRepository {
    fn read_model_query_capabilities(&self) -> ReadModelQueryCapabilities {
        self.model_store.read_model_query_capabilities()
    }

    fn load_graph(
        &self,
        request: ReadModelLoadRequest,
    ) -> impl Future<Output = Result<ReadModelLoadGraph, TableStoreError>> + Send + '_ {
        self.model_store.load_graph(request)
    }
}

impl SnapshotStore for InMemoryRepository {
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

    fn get_snapshots<'a>(
        &'a self,
        identities: &'a [StreamIdentity],
    ) -> impl Future<Output = Result<Vec<SnapshotRecord>, RepositoryError>> + Send + 'a {
        async move {
            let storage = self
                .snapshot_store
                .storage
                .read()
                .map_err(|_| RepositoryError::LockPoisoned("async snapshot read"))?;
            Ok(identities
                .iter()
                .filter_map(|identity| storage.get(&identity.storage_key()).cloned())
                .collect())
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
        repo: &InMemoryRepository,
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
        let repo = InMemoryRepository::new();
        assert!(repo.event_store.read().unwrap().is_empty());
    }

    #[tokio::test]
    async fn single_entity_commit() {
        let repo = InMemoryRepository::new();
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
        let repo = InMemoryRepository::new();

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
        let repo = InMemoryRepository::new();

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
        let repo = InMemoryRepository::new();

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
        let repo = InMemoryRepository::new();

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
