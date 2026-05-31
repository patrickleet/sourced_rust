#![expect(
    clippy::manual_async_fn,
    reason = "async trait impls return impl Future + Send to preserve public Send bounds"
)]

use std::future::Future;
use std::sync::Arc;

use crate::entity::Entity;
use crate::lock::{AsyncLock, AsyncLockManager, InMemoryAsyncLockManager};
use crate::read_model::{
    ReadModelAdapterCapabilities, ReadModelCommitOutcome, ReadModelError, ReadModelLoadGraph,
    ReadModelLoadRequest, ReadModelQueryCapabilities, ReadModelWritePlan,
};
use crate::repository::{
    AsyncCommitBatch, AsyncGetStream, AsyncInboxStore, AsyncReadModelWritePlanStore,
    AsyncRelationalReadModelQueryStore, AsyncSnapshotStore, AsyncTransactionalCommit,
    RepositoryError, StreamIdentity,
};
use crate::snapshot::SnapshotRecord;

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
pub struct QueuedRepository<R, L = InMemoryAsyncLockManager> {
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
            lock_manager: Arc::new(InMemoryAsyncLockManager::new()),
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
// Async variant (async lock manager): the same serialization semantics over
// the async repository trait surface. `QueuedRepository<R, AsyncLockManager>`
// is a drop-in async repository — `.queued_async().async_aggregate::<T>()`
// serializes per-aggregate `get`/`commit` exactly like the sync variant.
// ============================================================================

impl<R, L: AsyncLockManager> QueuedRepository<R, L> {
    /// Create a `QueuedRepository` with a custom async lock manager.
    pub fn with_async_lock_manager(inner: R, lock_manager: L) -> Self {
        QueuedRepository {
            inner,
            lock_manager: Arc::new(lock_manager),
        }
    }

    // Distinct names from the sync inherent helpers: coherence cannot prove a
    // type is not both a `LockManager` and an `AsyncLockManager`, so same-named
    // inherent methods across the two bounded impls would be ambiguous.
    fn ensure_async_lock(&self, id: &str) -> Result<Arc<L::Lock>, RepositoryError> {
        Ok(self.lock_manager.get_lock(id)?)
    }

    async fn lock_async_ids_in_order(
        &self,
        ids: &[&str],
    ) -> Result<Vec<Arc<L::Lock>>, RepositoryError> {
        let mut unique: Vec<&str> = ids.to_vec();
        unique.sort_unstable();
        unique.dedup();

        let mut locks = Vec::with_capacity(unique.len());
        for id in unique {
            let lock = self.ensure_async_lock(id)?;
            lock.lock().await?;
            locks.push(lock);
        }

        Ok(locks)
    }
}

impl<R, L> AsyncGetStream for QueuedRepository<R, L>
where
    R: AsyncGetStream,
    L: AsyncLockManager,
{
    fn get_stream<'a>(
        &'a self,
        identity: &'a StreamIdentity,
    ) -> impl Future<Output = Result<Option<Entity>, RepositoryError>> + Send + 'a {
        async move {
            // Acquire and HOLD the per-stream lock across the load, like the
            // synchronous `GetOne`. It is released by `commit_batch_async` on
            // success, or by an explicit `unlock`/`abort`.
            let lock = self.ensure_async_lock(&identity.storage_key())?;
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
            let _locks = self.lock_async_ids_in_order(&key_refs).await?;
            self.inner.get_streams(identities).await
        }
    }
}

impl<R, L> AsyncTransactionalCommit for QueuedRepository<R, L>
where
    R: AsyncTransactionalCommit,
    L: AsyncLockManager,
{
    fn commit_batch_async<'a>(
        &'a self,
        batch: AsyncCommitBatch<'a>,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a {
        async move {
            // Resolve the lock handles for the committed streams. Like the sync
            // `commit_batch`, this does not acquire (a prior locking load owns
            // them) and releases only after the inner commit succeeds, leaving
            // them held on error so callers can retry or `abort`.
            let mut locks = Vec::with_capacity(batch.streams.len());
            for stream in &batch.streams {
                locks.push(self.ensure_async_lock(&stream.identity.storage_key())?);
            }

            let result = self.inner.commit_batch_async(batch).await;

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

impl<R, L> AsyncSnapshotStore for QueuedRepository<R, L>
where
    R: AsyncSnapshotStore,
    L: AsyncLockManager,
{
    fn get_snapshot_async<'a>(
        &'a self,
        identity: &'a StreamIdentity,
    ) -> impl Future<Output = Result<Option<SnapshotRecord>, RepositoryError>> + Send + 'a {
        self.inner.get_snapshot_async(identity)
    }

    fn save_snapshot_async<'a>(
        &'a self,
        identity: &'a StreamIdentity,
        record: SnapshotRecord,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a {
        self.inner.save_snapshot_async(identity, record)
    }

    fn delete_snapshot_async<'a>(
        &'a self,
        identity: &'a StreamIdentity,
    ) -> impl Future<Output = Result<bool, RepositoryError>> + Send + 'a {
        self.inner.delete_snapshot_async(identity)
    }
}

impl<R, L> AsyncReadModelWritePlanStore for QueuedRepository<R, L>
where
    R: AsyncReadModelWritePlanStore,
    L: AsyncLockManager,
{
    fn read_model_capabilities_async(&self) -> ReadModelAdapterCapabilities {
        self.inner.read_model_capabilities_async()
    }

    fn commit_write_plan_async(
        &self,
        plan: ReadModelWritePlan,
    ) -> impl Future<Output = Result<ReadModelCommitOutcome, ReadModelError>> + Send + '_ {
        self.inner.commit_write_plan_async(plan)
    }
}

impl<R, L> AsyncRelationalReadModelQueryStore for QueuedRepository<R, L>
where
    R: AsyncRelationalReadModelQueryStore,
    L: AsyncLockManager,
{
    fn read_model_query_capabilities_async(&self) -> ReadModelQueryCapabilities {
        self.inner.read_model_query_capabilities_async()
    }

    fn load_graph_async(
        &self,
        request: ReadModelLoadRequest,
    ) -> impl Future<Output = Result<ReadModelLoadGraph, ReadModelError>> + Send + '_ {
        self.inner.load_graph_async(request)
    }
}

impl<R, L> AsyncInboxStore for QueuedRepository<R, L>
where
    R: AsyncInboxStore,
    L: AsyncLockManager,
{
    fn inbox_contains_async<'a>(
        &'a self,
        consumer: &'a str,
        message_id: &'a str,
    ) -> impl Future<Output = Result<bool, RepositoryError>> + Send + 'a {
        self.inner.inbox_contains_async(consumer, message_id)
    }
}

/// Async opt-out reads for a queued repository — the async counterpart to
/// [`GetWithOpts`]. `ReadOpts::no_lock()` reads without acquiring the lock.
pub trait AsyncGetWithOpts {
    fn get_stream_with<'a>(
        &'a self,
        identity: &'a StreamIdentity,
        opts: ReadOpts,
    ) -> impl Future<Output = Result<Option<Entity>, RepositoryError>> + Send + 'a;
}

/// Async opt-out multi-reads — the async counterpart to [`GetAllWithOpts`].
pub trait AsyncGetAllWithOpts {
    fn get_streams_with<'a>(
        &'a self,
        identities: &'a [StreamIdentity],
        opts: ReadOpts,
    ) -> impl Future<Output = Result<Vec<Entity>, RepositoryError>> + Send + 'a;
}

impl<R, L> AsyncGetWithOpts for QueuedRepository<R, L>
where
    R: AsyncGetStream,
    L: AsyncLockManager,
{
    fn get_stream_with<'a>(
        &'a self,
        identity: &'a StreamIdentity,
        opts: ReadOpts,
    ) -> impl Future<Output = Result<Option<Entity>, RepositoryError>> + Send + 'a {
        async move {
            if opts.lock {
                let lock = self.ensure_async_lock(&identity.storage_key())?;
                lock.lock().await?;
            }
            self.inner.get_stream(identity).await
        }
    }
}

impl<R, L> AsyncGetAllWithOpts for QueuedRepository<R, L>
where
    R: AsyncGetStream,
    L: AsyncLockManager,
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
                let _locks = self.lock_async_ids_in_order(&key_refs).await?;
            }
            self.inner.get_streams(identities).await
        }
    }
}

/// Releasing a held lock for an aborted load over the async repository surface.
///
/// Release is `async` because a durable [`AsyncLock`] (e.g. the SQLx lease-table
/// locks) releases by talking to its backing store; the in-memory lock resolves
/// immediately. It is a separate trait because coherence cannot prove a type is
/// not both several lock-manager kinds at once.
pub trait AsyncUnlockableRepository: Send + Sync {
    /// Release the lock held for a stream.
    ///
    /// Keyed by [`StreamIdentity`] — the same key the locking `AsyncGetStream`
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

impl<R, L: AsyncLockManager> AsyncUnlockableRepository for QueuedRepository<R, L>
where
    R: Send + Sync,
{
    fn unlock<'a>(
        &'a self,
        identity: &'a StreamIdentity,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a {
        async move {
            self.ensure_async_lock(&identity.storage_key())?
                .unlock()
                .await?;
            Ok(())
        }
    }
}

/// Builder trait for wrapping a repository with queue locking.
pub trait Queueable: Sized {
    /// Wrap with the default async lock manager. Pair with
    /// `.async_aggregate::<T>()` for per-aggregate serialization over the async
    /// repository surface.
    fn queued_async(self) -> QueuedRepository<Self, InMemoryAsyncLockManager> {
        QueuedRepository::with_async_lock_manager(self, InMemoryAsyncLockManager::new())
    }

    /// Wrap with a custom async lock manager.
    fn queued_async_with<L: AsyncLockManager>(self, lock_manager: L) -> QueuedRepository<Self, L> {
        QueuedRepository::with_async_lock_manager(self, lock_manager)
    }
}

impl<T> Queueable for T {}
