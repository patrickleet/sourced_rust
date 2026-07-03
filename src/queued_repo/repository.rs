#![expect(
    clippy::manual_async_fn,
    reason = "async trait impls return impl Future + Send to preserve public Send bounds"
)]

use std::future::Future;
use std::sync::Arc;

use crate::entity::Entity;
use crate::lock::{InMemoryLockManager, Lock, LockManager};
use crate::read_model::{ReadModelLoadGraph, ReadModelLoadRequest, ReadModelQueryCapabilities};
use crate::repository::{
    CommitBatch, GetStream, InboxStore, ReadModelWritePlanStore, RelationalReadModelQueryStore,
    RepositoryError, SnapshotStore, StreamIdentity, TransactionalCommit,
};
use crate::snapshot::SnapshotRecord;
use crate::table::{TableAdapterCapabilities, TableCommitOutcome, TableStoreError, TableWritePlan};

/// Options for read operations.
#[derive(Debug, Clone, Copy)]
pub struct ReadOpts {
    /// Whether to acquire a lock on the entity/entities.
    pub lock: bool,
}

impl Default for ReadOpts {
    fn default() -> Self {
        Self { lock: true }
    }
}

impl ReadOpts {
    /// Create options that skip locking.
    pub fn no_lock() -> Self {
        Self { lock: false }
    }
}

/// Repository wrapper that serializes access with process-local per-stream locks.
///
/// Locking reads (`get` and `get_many`) intentionally keep matching locks held
/// after returning. Call `commit` to release those locks after a successful
/// write, or call `abort`/`unlock` when the loaded entity is no longer being
/// written. Dropping a loaded entity without `commit` or `abort` leaves its
/// in-memory lock held until an explicit unlock.
///
/// Commit releases held locks only after the inner repository succeeds. On
/// commit errors, locks remain held so callers can inspect state, retry, or
/// explicitly abort.
pub struct QueuedRepository<R, L = InMemoryLockManager> {
    inner: R,
    lock_manager: Arc<L>,
}

impl<R: Clone, L> Clone for QueuedRepository<R, L> {
    fn clone(&self) -> Self {
        QueuedRepository {
            inner: self.inner.clone(),
            lock_manager: Arc::clone(&self.lock_manager),
        }
    }
}

impl<R> QueuedRepository<R> {
    pub fn new(inner: R) -> Self {
        QueuedRepository {
            inner,
            lock_manager: Arc::new(InMemoryLockManager::new()),
        }
    }
}

impl<R, L> QueuedRepository<R, L> {
    /// Access the inner repository.
    pub fn inner(&self) -> &R {
        &self.inner
    }

    /// Access the lock manager.
    pub fn lock_manager(&self) -> &L {
        &self.lock_manager
    }
}

// ============================================================================
// Queue-locking variant over the repository trait surface. `.queued().aggregate::<T>()`
// serializes per-aggregate `get`/`commit` with the configured async lock manager.
// ============================================================================

impl<R, L: LockManager> QueuedRepository<R, L> {
    /// Create a `QueuedRepository` with a custom async lock manager.
    pub fn with_lock_manager(inner: R, lock_manager: L) -> Self {
        QueuedRepository {
            inner,
            lock_manager: Arc::new(lock_manager),
        }
    }

    fn ensure_lock(&self, id: &str) -> Result<Arc<L::Lock>, RepositoryError> {
        Ok(self.lock_manager.get_lock(id)?)
    }

    async fn lock_ids_in_order(&self, ids: &[&str]) -> Result<Vec<Arc<L::Lock>>, RepositoryError> {
        let mut unique: Vec<&str> = ids.to_vec();
        unique.sort_unstable();
        unique.dedup();

        let mut locks = Vec::with_capacity(unique.len());
        for id in unique {
            let lock = self.ensure_lock(id)?;
            lock.lock().await?;
            locks.push(lock);
        }

        Ok(locks)
    }
}

impl<R, L> GetStream for QueuedRepository<R, L>
where
    R: GetStream,
    L: LockManager,
{
    fn get_stream<'a>(
        &'a self,
        identity: &'a StreamIdentity,
    ) -> impl Future<Output = Result<Option<Entity>, RepositoryError>> + Send + 'a {
        async move {
            // Acquire and HOLD the per-stream lock across the load, like the
            // synchronous `GetOne`. It is released by `commit_batch` on
            // success, or by an explicit `unlock`/`abort`.
            let lock = self.ensure_lock(&identity.storage_key())?;
            lock.lock().await?;
            self.inner.get_stream(identity).await
        }
    }

    fn get_streams<'a>(
        &'a self,
        identities: &'a [StreamIdentity],
    ) -> impl Future<Output = Result<Vec<Entity>, RepositoryError>> + Send + 'a {
        async move {
            let keys: Vec<String> = identities.iter().map(StreamIdentity::storage_key).collect();
            let key_refs: Vec<&str> = keys.iter().map(String::as_str).collect();
            // Sorted-order acquire prevents deadlock; locks held after return.
            let _locks = self.lock_ids_in_order(&key_refs).await?;
            self.inner.get_streams(identities).await
        }
    }

    fn get_stream_tail<'a>(
        &'a self,
        identity: &'a StreamIdentity,
        after_version: u64,
    ) -> impl Future<Output = Result<Option<Entity>, RepositoryError>> + Send + 'a {
        async move {
            // Acquire and HOLD the per-stream lock across the load, exactly like
            // `get_stream` — the snapshot tail load is still a load and must
            // serialize against concurrent writers. Released by `commit_batch`
            // on success, or by `unlock`/`abort`.
            let lock = self.ensure_lock(&identity.storage_key())?;
            lock.lock().await?;
            self.inner.get_stream_tail(identity, after_version).await
        }
    }
}

impl<R, L> TransactionalCommit for QueuedRepository<R, L>
where
    R: TransactionalCommit,
    L: LockManager,
{
    fn commit_batch<'a>(
        &'a self,
        batch: CommitBatch<'a>,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a {
        async move {
            // Resolve the lock handles for the committed streams. Like the sync
            // `commit_batch`, this does not acquire (a prior locking load owns
            // them) and releases only after the inner commit succeeds, leaving
            // them held on error so callers can retry or `abort`.
            let mut locks = Vec::with_capacity(batch.streams.len());
            for stream in &batch.streams {
                locks.push(self.ensure_lock(&stream.identity.storage_key())?);
            }

            let result = self.inner.commit_batch(batch).await;

            if result.is_ok() {
                // Best-effort release: the inner commit already succeeded, so a
                // lock-cleanup failure must not fail the committed write (a durable
                // lease is reclaimed by its TTL regardless). Mirrors the
                // best-effort release in the lock layer itself.
                for lock in locks {
                    let _ = lock.unlock().await;
                }
            }

            result
        }
    }
}

// Non-locking forwards: read models, snapshots, and the consumer inbox are not
// gated by aggregate locks (matching the sync `SnapshotStore` delegation), so a
// queued repository stays a complete drop-in for its inner async repository.

impl<R, L> SnapshotStore for QueuedRepository<R, L>
where
    R: SnapshotStore,
    L: LockManager,
{
    fn get_snapshot<'a>(
        &'a self,
        identity: &'a StreamIdentity,
    ) -> impl Future<Output = Result<Option<SnapshotRecord>, RepositoryError>> + Send + 'a {
        self.inner.get_snapshot(identity)
    }

    fn get_snapshots<'a>(
        &'a self,
        identities: &'a [StreamIdentity],
    ) -> impl Future<Output = Result<Vec<SnapshotRecord>, RepositoryError>> + Send + 'a {
        self.inner.get_snapshots(identities)
    }

    fn save_snapshot<'a>(
        &'a self,
        identity: &'a StreamIdentity,
        record: SnapshotRecord,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a {
        self.inner.save_snapshot(identity, record)
    }

    fn delete_snapshot<'a>(
        &'a self,
        identity: &'a StreamIdentity,
    ) -> impl Future<Output = Result<bool, RepositoryError>> + Send + 'a {
        self.inner.delete_snapshot(identity)
    }
}

impl<R, L> ReadModelWritePlanStore for QueuedRepository<R, L>
where
    R: ReadModelWritePlanStore,
    L: LockManager,
{
    fn read_model_capabilities(&self) -> TableAdapterCapabilities {
        self.inner.read_model_capabilities()
    }

    fn commit_write_plan(
        &self,
        plan: TableWritePlan,
    ) -> impl Future<Output = Result<TableCommitOutcome, TableStoreError>> + Send + '_ {
        self.inner.commit_write_plan(plan)
    }
}

impl<R, L> RelationalReadModelQueryStore for QueuedRepository<R, L>
where
    R: RelationalReadModelQueryStore,
    L: LockManager,
{
    fn read_model_query_capabilities(&self) -> ReadModelQueryCapabilities {
        self.inner.read_model_query_capabilities()
    }

    fn load_graph(
        &self,
        request: ReadModelLoadRequest,
    ) -> impl Future<Output = Result<ReadModelLoadGraph, TableStoreError>> + Send + '_ {
        self.inner.load_graph(request)
    }
}

impl<R, L> InboxStore for QueuedRepository<R, L>
where
    R: InboxStore,
    L: LockManager,
{
    fn inbox_contains<'a>(
        &'a self,
        consumer: &'a str,
        message_id: &'a str,
    ) -> impl Future<Output = Result<bool, RepositoryError>> + Send + 'a {
        self.inner.inbox_contains(consumer, message_id)
    }

    fn purge_inbox_older_than(
        &self,
        age: std::time::Duration,
    ) -> impl Future<Output = Result<u64, RepositoryError>> + Send {
        // Inbox maintenance is a direct store operation; the queue/lock layer adds
        // nothing, so delegate straight through to the inner store.
        self.inner.purge_inbox_older_than(age)
    }
}

/// Opt-out reads for a queued repository — the counterpart to
/// [`GetWithOpts`]. `ReadOpts::no_lock()` reads without acquiring the lock.
pub trait GetWithOpts {
    fn get_stream_with<'a>(
        &'a self,
        identity: &'a StreamIdentity,
        opts: ReadOpts,
    ) -> impl Future<Output = Result<Option<Entity>, RepositoryError>> + Send + 'a;
}

/// Opt-out multi-reads — the counterpart to [`GetAllWithOpts`].
pub trait GetAllWithOpts {
    fn get_streams_with<'a>(
        &'a self,
        identities: &'a [StreamIdentity],
        opts: ReadOpts,
    ) -> impl Future<Output = Result<Vec<Entity>, RepositoryError>> + Send + 'a;
}

impl<R, L> GetWithOpts for QueuedRepository<R, L>
where
    R: GetStream,
    L: LockManager,
{
    fn get_stream_with<'a>(
        &'a self,
        identity: &'a StreamIdentity,
        opts: ReadOpts,
    ) -> impl Future<Output = Result<Option<Entity>, RepositoryError>> + Send + 'a {
        async move {
            if opts.lock {
                let lock = self.ensure_lock(&identity.storage_key())?;
                lock.lock().await?;
            }
            self.inner.get_stream(identity).await
        }
    }
}

impl<R, L> GetAllWithOpts for QueuedRepository<R, L>
where
    R: GetStream,
    L: LockManager,
{
    fn get_streams_with<'a>(
        &'a self,
        identities: &'a [StreamIdentity],
        opts: ReadOpts,
    ) -> impl Future<Output = Result<Vec<Entity>, RepositoryError>> + Send + 'a {
        async move {
            if opts.lock {
                let keys: Vec<String> =
                    identities.iter().map(StreamIdentity::storage_key).collect();
                let key_refs: Vec<&str> = keys.iter().map(String::as_str).collect();
                let _locks = self.lock_ids_in_order(&key_refs).await?;
            }
            self.inner.get_streams(identities).await
        }
    }
}

/// Releasing a held lock for an aborted load over the async repository surface.
///
/// Release is `async` because a durable [`Lock`] (e.g. the SQLx lease-table
/// locks) releases by talking to its backing store; the in-memory lock resolves
/// immediately. It is a separate trait because coherence cannot prove a type is
/// not both several lock-manager kinds at once.
pub trait UnlockableRepository: Send + Sync {
    /// Release the lock held for a stream.
    ///
    /// Keyed by [`StreamIdentity`] — the same key the locking `GetStream`
    /// reads acquire — so an aborted load releases exactly the lock it took.
    fn unlock<'a>(
        &'a self,
        identity: &'a StreamIdentity,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a;

    /// Release a lock for an aborted load (alias for [`unlock`](Self::unlock)).
    fn abort<'a>(
        &'a self,
        identity: &'a StreamIdentity,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a {
        self.unlock(identity)
    }
}

impl<R, L: LockManager> UnlockableRepository for QueuedRepository<R, L>
where
    R: Send + Sync,
{
    fn unlock<'a>(
        &'a self,
        identity: &'a StreamIdentity,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a {
        async move {
            self.ensure_lock(&identity.storage_key())?.unlock().await?;
            Ok(())
        }
    }
}

/// Builder trait for wrapping a repository with queue locking.
pub trait Queueable: Sized {
    /// Wrap with the default async lock manager. Pair with
    /// `.aggregate::<T>()` for per-aggregate serialization over the async
    /// repository surface.
    fn queued(self) -> QueuedRepository<Self, InMemoryLockManager> {
        QueuedRepository::with_lock_manager(self, InMemoryLockManager::new())
    }

    /// Wrap with a custom async lock manager.
    fn queued_with<L: LockManager>(self, lock_manager: L) -> QueuedRepository<Self, L> {
        QueuedRepository::with_lock_manager(self, lock_manager)
    }
}

impl<T> Queueable for T {}
