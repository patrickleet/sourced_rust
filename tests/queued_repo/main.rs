//! `QueuedRepository` — per-aggregate serialization over the repository
//! surface. Proves `.queued().aggregate::<T>()` engages the async lock on
//! `get`/`commit`, plus per-aggregate granularity, the `no_lock` opt-out,
//! and explicit abort.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use distributed::{
    sourced, Aggregate, AggregateBuilder, AggregateRepository, Entity, GetStream,
    HashMapRepository, InMemoryLockManager, Queueable, StreamIdentity,
};
use tokio::sync::Barrier;

#[derive(Default)]
struct Counter {
    entity: Entity,
    value: i32,
}

#[sourced(entity, aggregate_type = "queued.counter")]
impl Counter {
    #[event("initialized")]
    fn create(&mut self, id: String) {
        self.entity.set_id(&id);
    }

    #[event("incremented")]
    fn increment(&mut self, id: String, by: i32) {
        self.entity.set_id(&id);
        self.value += by;
    }
}

type QueuedCounterRepo = AggregateRepository<
    distributed::QueuedRepository<HashMapRepository, InMemoryLockManager>,
    Counter,
>;

fn queued_repo() -> Arc<QueuedCounterRepo> {
    Arc::new(HashMapRepository::new().queued().aggregate::<Counter>())
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn queued_repo_n_writers_commit_in_fifo_order() {
    // N writers race to increment the SAME aggregate. The queued repo serializes
    // load→commit under a per-stream async lock, so the writes must apply one at
    // a time. We pin two invariants:
    //   1. The final version is exactly N (no lost updates, no double-applies).
    //   2. The committed event order equals the lock-grant order — each writer
    //      records the order in which it *acquired* the lock (the instant its
    //      `get` returned), and stamps that grant index into its event payload.
    //      Replaying the stream must then reproduce that exact grant order.
    const WRITERS: usize = 10;

    let base = HashMapRepository::new();
    let reader = base.clone();
    let repo: Arc<QueuedCounterRepo> = Arc::new(base.queued().aggregate::<Counter>());

    // Seed the aggregate so every writer takes the load-modify-commit path.
    {
        let mut counter = Counter::default();
        counter.create("fifo".into()).unwrap();
        repo.commit(&mut counter).await.unwrap();
    }

    // Shared log of the order in which writers were granted the lock.
    let grant_order = Arc::new(Mutex::new(Vec::<usize>::new()));
    // Release all writers together so they genuinely contend for the lock.
    let barrier = Arc::new(Barrier::new(WRITERS));

    let mut handles = Vec::with_capacity(WRITERS);
    for writer in 0..WRITERS {
        let repo = Arc::clone(&repo);
        let grant_order = Arc::clone(&grant_order);
        let barrier = Arc::clone(&barrier);
        handles.push(tokio::spawn(async move {
            barrier.wait().await;
            // `get` parks on the held lock until granted; the instant it returns
            // is this writer's place in the grant order.
            let mut counter = repo.get("fifo").await.unwrap().unwrap();
            let grant_index = {
                let mut order = grant_order.lock().unwrap();
                order.push(writer);
                order.len() - 1
            };
            // Stamp the grant index into the event so the stream records order.
            counter
                .increment("fifo".into(), grant_index as i32)
                .unwrap();
            repo.commit(&mut counter).await.unwrap();
        }));
    }
    for handle in handles {
        handle.await.unwrap();
    }

    // Invariant 1: exactly N increments landed.
    let identity = StreamIdentity::new(Counter::aggregate_type(), "fifo").unwrap();
    let entity = reader
        .get_stream(&identity)
        .await
        .unwrap()
        .expect("stream should exist");
    // version = 1 (create) + WRITERS increments.
    assert_eq!(
        entity.committed_version() as usize,
        WRITERS + 1,
        "every writer's increment must land exactly once (no lost or doubled writes)"
    );

    // Invariant 2: the increment events, in stream order, carry grant indices in
    // ascending 0..WRITERS — i.e. the stream order equals the lock-grant order.
    let grant_indices: Vec<i32> = entity
        .events()
        .iter()
        .filter(|event| event.event_name == "incremented")
        .map(|event| {
            let (_, by): (String, i32) =
                bitcode::deserialize(&event.payload).expect("payload should decode");
            by
        })
        .collect();
    let expected: Vec<i32> = (0..WRITERS as i32).collect();
    assert_eq!(
        grant_indices, expected,
        "committed event order must match the lock-grant order (strict FIFO serialization)"
    );

    let recorded_grant_order = grant_order.lock().unwrap().clone();
    assert_eq!(
        recorded_grant_order.len(),
        WRITERS,
        "every writer must have been granted the lock exactly once"
    );
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
