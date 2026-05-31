//! Async `QueuedRepository` — per-aggregate serialization over the async
//! repository surface. Proves `.queued_async().async_aggregate::<T>()` engages
//! the async lock on `get`/`commit` exactly like the sync `.queued()` path,
//! plus per-aggregate granularity, the `no_lock` opt-out, and explicit abort.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use sourced_rust::{
    sourced, AsyncAggregateBuilder, AsyncAggregateRepository, Entity, HashMapRepository,
    InMemoryAsyncLockManager, Queueable,
};

#[derive(Default)]
struct Counter {
    entity: Entity,
    value: i32,
}

#[sourced(entity, aggregate_type = "queued.counter")]
impl Counter {
    #[event("Created")]
    fn create(&mut self, id: String) {
        self.entity.set_id(&id);
    }

    #[event("Incremented")]
    fn increment(&mut self, id: String, by: i32) {
        self.entity.set_id(&id);
        self.value += by;
    }
}

type QueuedCounterRepo = AsyncAggregateRepository<
    sourced_rust::QueuedRepository<HashMapRepository, InMemoryAsyncLockManager>,
    Counter,
>;

fn queued_repo() -> Arc<QueuedCounterRepo> {
    Arc::new(
        HashMapRepository::new()
            .queued_async()
            .async_aggregate::<Counter>(),
    )
}

async fn seed(repo: &QueuedCounterRepo, id: &str) {
    let mut counter = Counter::default();
    counter.create(id.into()).unwrap();
    // A bare commit (no prior locking load) takes the lock handle and releases
    // it on success — it does not deadlock and leaves the lock free.
    repo.commit(&mut counter).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn second_load_waits_until_first_commits() {
    let repo = queued_repo();
    seed(&repo, "c1").await;

    // Task 1 loads (acquires + HOLDS the per-stream lock).
    let mut held = repo.get("c1").await.unwrap().unwrap();
    held.increment("c1".into(), 5).unwrap();

    // Task 2 tries to load the same aggregate; it must park on the held lock.
    let acquired = Arc::new(AtomicBool::new(false));
    let task_repo = Arc::clone(&repo);
    let task_flag = Arc::clone(&acquired);
    let task2 = tokio::spawn(async move {
        let loaded = task_repo.get("c1").await.unwrap().unwrap();
        task_flag.store(true, Ordering::SeqCst);
        loaded
    });

    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !acquired.load(Ordering::SeqCst),
        "the second load must block while the first holds the lock"
    );

    // Committing the first load releases the lock, unblocking task 2.
    repo.commit(&mut held).await.unwrap();
    let loaded = task2.await.unwrap();

    assert!(acquired.load(Ordering::SeqCst));
    // Task 2 loaded AFTER the commit, so it observes the first increment.
    assert_eq!(
        loaded.value, 5,
        "serialized load sees the prior writer's committed state"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn distinct_aggregates_do_not_block_each_other() {
    let repo = queued_repo();
    seed(&repo, "a").await;
    seed(&repo, "b").await;

    // Hold the lock on "a".
    let _held_a = repo.get("a").await.unwrap().unwrap();

    // Loading a different aggregate must not wait on "a"'s lock.
    let got_b = tokio::time::timeout(Duration::from_millis(500), repo.get("b"))
        .await
        .expect("loading a distinct aggregate must not block")
        .unwrap();
    assert!(got_b.is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn peek_reads_without_acquiring_the_lock() {
    let repo = queued_repo();
    seed(&repo, "c1").await;

    // Hold the lock on "c1".
    let _held = repo.get("c1").await.unwrap().unwrap();

    // A no-lock peek of the same aggregate must not block.
    let peeked = tokio::time::timeout(Duration::from_millis(500), repo.peek("c1"))
        .await
        .expect("no_lock peek must not block on a held lock")
        .unwrap();
    assert!(peeked.is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn abort_releases_a_held_lock() {
    let repo = queued_repo();
    seed(&repo, "c1").await;

    // Load (acquire) then abort without committing.
    let held = repo.get("c1").await.unwrap().unwrap();
    repo.abort(&held).await.unwrap();

    // The lock is free again: a subsequent load must not block.
    let reloaded = tokio::time::timeout(Duration::from_millis(500), repo.get("c1"))
        .await
        .expect("load after abort must not block")
        .unwrap();
    assert!(reloaded.is_some());
}
