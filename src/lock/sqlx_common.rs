//! Shared, dialect-neutral machinery for the SQLx lease-table locks.
//!
//! The [`PostgresLock`](super::PostgresLock) / [`SqliteLock`](super::SqliteLock)
//! differ only in their SQL (placeholder style + the database-clock function)
//! and pool type. Everything else — the in-process gate, owner-token minting,
//! the acquire poll loop, and release — lives here so both backends share one
//! implementation.
//!
//! ## Model
//!
//! Each per-key lock layers an **in-process async gate** ([`InMemoryAsyncLock`])
//! over a **durable DB lease** (a row in `aggregate_locks`). The gate serializes
//! same-process tasks with true wakeups (no DB polling between them); only the
//! local gate winner contends on the database, and only against *other
//! processes*. The DB lease carries an `owner_token` (this acquisition's
//! identity) and an `expires_at` derived from the **database clock** (so there is
//! one authoritative clock — no cross-process skew). A lease is stealable once
//! it expires; release is scoped to the owner token so it never stomps a holder
//! that legitimately reclaimed an expired lease.
//!
//! This is a mutual-exclusion *optimization*, not a fencing guarantee: a critical
//! section that outlives `lease_ttl` can be stolen while the original holder
//! still believes it holds the lock. That is safe here only because the event
//! store's `(aggregate_type, aggregate_id, sequence)` primary key is the true
//! concurrency boundary — a stale writer fails its optimistic commit rather than
//! corrupting data. v1 has no lease renewal: set `lease_ttl` above the worst-case
//! critical section.

use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::{AsyncLock, InMemoryAsyncLock, LockError};

/// Tunables for a lease lock manager. Shared by every per-key lock it hands out.
#[derive(Debug, Clone)]
pub(crate) struct LeaseConfig {
    /// How long an acquired lease stays valid before it becomes stealable.
    pub lease_ttl: Duration,
    /// How long to wait between contended acquire attempts.
    pub retry_interval: Duration,
    /// Optional cap on how long [`lease_lock`] waits before giving up. `None`
    /// waits indefinitely (matching the in-memory lock); the `lease_ttl` still
    /// bounds the wait on a *crashed* holder.
    pub max_wait: Option<Duration>,
}

impl Default for LeaseConfig {
    fn default() -> Self {
        Self {
            lease_ttl: Duration::from_secs(30),
            retry_interval: Duration::from_millis(50),
            max_wait: None,
        }
    }
}

/// Per-key lock state shared between the acquiring and releasing handles.
///
/// `QueuedRepository` acquires on a locking `get` and releases from a *different*
/// `get_lock(key)` handle on commit/abort; the manager returns the same cached
/// `Arc`, so the gate and the owner token must live here, not in transient
/// handle locals — mirroring `InMemoryAsyncLock`'s manager-owned state.
pub(crate) struct LockShared {
    /// In-process gate: same-process tasks serialize here (true wakeups, no DB
    /// polling) so only one local task contends on the DB lease at a time.
    pub gate: InMemoryAsyncLock,
    /// The owner token of the lease currently held by this process, if any.
    pub token: Mutex<Option<String>>,
}

impl LockShared {
    pub fn new() -> Self {
        Self {
            gate: InMemoryAsyncLock::new(),
            token: Mutex::new(None),
        }
    }
}

/// A dialect-specific lease backend. Implemented by the per-backend locks; the
/// shared [`lease_lock`]/[`lease_try_lock`]/[`lease_unlock`] drive it.
pub(crate) trait LeaseBackend: Send + Sync {
    fn shared(&self) -> &LockShared;
    fn config(&self) -> &LeaseConfig;
    /// Mint a globally-unique token for a new acquisition attempt.
    fn mint_token(&self) -> String;
    /// One atomic conditional-acquire of the lease for the given candidate token.
    /// `Ok(true)` = acquired (or re-acquired our own token); `Ok(false)` =
    /// contended (held by another, not expired) or a transient busy condition.
    fn db_acquire(&self, token: &str) -> impl Future<Output = Result<bool, LockError>> + Send;
    /// Release the lease iff it still carries our token (best-effort).
    fn db_release(&self, token: &str) -> impl Future<Output = Result<(), LockError>> + Send;
}

/// Releases the in-process gate synchronously on drop, unless disarmed.
///
/// Cancellation safety: the lease helpers hold the gate across `.await` points
/// (DB acquire, retry sleep, DB release). If the caller's future is dropped
/// there, only `Drop` runs — so the gate MUST be released from `Drop`, or the
/// key wedges for every later same-process acquire. `Drop` cannot `.await`, so
/// it uses the gate's synchronous `unlock_core`. On the success path the holder
/// keeps the gate, so [`disarm`](Self::disarm) suppresses the release.
struct GateGuard<'a> {
    gate: &'a InMemoryAsyncLock,
    armed: bool,
}

impl<'a> GateGuard<'a> {
    fn new(gate: &'a InMemoryAsyncLock) -> Self {
        Self { gate, armed: true }
    }

    /// Keep the gate held (the acquisition succeeded); the eventual `unlock`
    /// releases it.
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for GateGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            let _ = self.gate.unlock_core();
        }
    }
}

/// Acquire: hold the in-process gate, then poll the DB lease until won or
/// `max_wait` elapses. The gate is released (via [`GateGuard`]) on every failure
/// path AND on cancellation. `max_wait` is measured from entry, so it also caps
/// the wait for the in-process gate, not just the DB polling.
pub(crate) async fn lease_lock(backend: &impl LeaseBackend) -> Result<(), LockError> {
    let started = Instant::now();
    let max_wait = backend.config().max_wait;

    // Acquire the in-process gate, bounding the wait by `max_wait` if set.
    match max_wait {
        Some(max) => match tokio::time::timeout(max, backend.shared().gate.lock()).await {
            Ok(result) => result?,
            Err(_elapsed) => {
                return Err(LockError::AcquireFailed(format!(
                    "lease acquire timed out after {max:?} waiting for the in-process gate"
                )))
            }
        },
        None => backend.shared().gate.lock().await?,
    }
    let mut guard = GateGuard::new(&backend.shared().gate);

    // One token per acquisition, reused across retries so a lost-response retry
    // can re-acquire our own row (the `OR owner_token = ours` upsert branch).
    let token = backend.mint_token();
    loop {
        match backend.db_acquire(&token).await {
            Ok(true) => {
                store_token(backend.shared(), Some(token));
                guard.disarm(); // success: keep the gate held until `unlock`
                return Ok(());
            }
            Ok(false) => {}
            Err(err) => return Err(err), // guard releases the gate on drop
        }
        if let Some(max) = max_wait {
            if started.elapsed() >= max {
                return Err(LockError::AcquireFailed(format!(
                    "lease acquire timed out after {max:?}"
                )));
            }
        }
        tokio::time::sleep(jittered(backend.config().retry_interval, &token)).await;
    }
}

/// Non-blocking acquire: take the in-process gate if free, then a single DB
/// acquire attempt. The gate is released (via [`GateGuard`]) unless we win.
pub(crate) async fn lease_try_lock(backend: &impl LeaseBackend) -> Result<bool, LockError> {
    if !backend.shared().gate.try_lock().await? {
        return Ok(false);
    }
    let mut guard = GateGuard::new(&backend.shared().gate);
    let token = backend.mint_token();
    match backend.db_acquire(&token).await {
        Ok(true) => {
            store_token(backend.shared(), Some(token));
            guard.disarm();
            Ok(true)
        }
        Ok(false) => Ok(false), // guard releases the gate on drop
        Err(err) => Err(err),   // guard releases the gate on drop
    }
}

/// Release: delete our lease row (best-effort — the lease TTL reclaims it if this
/// fails, and a committed write must never fail on lock cleanup). The in-process
/// gate is released by [`GateGuard`] on drop, so it is freed even if this future
/// is cancelled mid `db_release`. Idempotent if we no longer hold the lease.
pub(crate) async fn lease_unlock(backend: &impl LeaseBackend) -> Result<(), LockError> {
    // Always-armed: this releases the gate on drop whether `db_release` completes,
    // errors, or the future is cancelled while awaiting it.
    let _guard = GateGuard::new(&backend.shared().gate);
    let token = store_token(backend.shared(), None);
    if let Some(token) = token {
        let _ = backend.db_release(&token).await;
    }
    Ok(())
}

/// Swap the stored owner token, returning the previous value. Used to set the
/// token on acquire and take it on release.
///
/// Infallible: the slot holds only an `Option<String>` and is mutated without
/// running user code, so the mutex can never truly be poisoned. We recover the
/// guard defensively rather than return an error.
fn store_token(shared: &LockShared, next: Option<String>) -> Option<String> {
    let mut slot = shared
        .token
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    std::mem::replace(&mut slot, next)
}

/// Per-process owner id: `"{prefix}-{pid}-{nanos}-{mgr_seq}"`. Distinct pids and
/// high-resolution construction time make it unique across processes/restarts;
/// the manager sequence disambiguates managers built in the same process.
pub(crate) fn default_owner_id(prefix: &str) -> String {
    static MANAGER_SEQ: AtomicU64 = AtomicU64::new(0);
    let pid = std::process::id();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seq = MANAGER_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{pid}-{nanos}-{seq}")
}

/// Mint a per-acquisition token unique within this manager: `"{owner_id}:{n}"`.
pub(crate) fn mint_token(owner_id: &str, seq: &AtomicU64) -> String {
    format!("{owner_id}:{}", seq.fetch_add(1, Ordering::Relaxed))
}

/// Spread cross-process waiters by adding up to +25% deterministic jitter
/// derived from the owner token, so distinct holders do not poll in lockstep.
fn jittered(base: Duration, token: &str) -> Duration {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    token.hash(&mut hasher);
    let frac = (hasher.finish() % 1000) as f64 / 1000.0;
    base + base.mul_f64(0.25 * frac)
}

/// Map an acquire query error to a `LockError`.
pub(crate) fn lease_acquire_error(err: sqlx::Error) -> LockError {
    LockError::AcquireFailed(format!("sqlx lease acquire failed: {err}"))
}

/// Map a release query error to a `LockError`.
pub(crate) fn lease_release_error(err: sqlx::Error) -> LockError {
    LockError::ReleaseFailed(format!("sqlx lease release failed: {err}"))
}
