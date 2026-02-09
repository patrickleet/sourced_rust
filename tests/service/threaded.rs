//! Threaded domain service tests — background listener dispatching commands.

use std::thread;
use std::time::Duration;

use sourced_rust::{
    CommitAggregate, DomainService, DomainServiceThread, GetAggregate, HandlerError,
    HashMapRepository, Message,
};
use sourced_rust::bus::{InMemoryQueue, Sender};

use crate::support::{Counter, CreateCounter, IncrementCounter};

// ============================================================================
// Test 1: Service thread processes a single command from queue
// ============================================================================

#[test]
fn service_thread_processes_single_command() {
    let queue = InMemoryQueue::new();
    let repo = HashMapRepository::new();

    let service = DomainService::new(repo.clone()).command("counter.create", |repo, msg| {
        let data: CreateCounter = msg.decode()?;
        let mut counter = Counter::default();
        counter.create(data.id);
        repo.commit_aggregate(&mut counter)?;
        Ok(())
    });

    let handle = DomainServiceThread::spawn(
        service,
        "counters".to_string(),
        queue.clone(),
        Duration::from_millis(10),
    );

    // Send a command to the queue
    let msg = Message::encode("cmd-1", "counter.create", &CreateCounter { id: "c1".into() })
        .unwrap();
    queue.send("counters", msg).unwrap();

    // Wait for processing
    thread::sleep(Duration::from_millis(200));

    // Verify aggregate was created
    let counter: Counter = repo.get_aggregate("c1").unwrap().unwrap();
    assert_eq!(counter.value, 0);

    let stats = handle.stop();
    assert_eq!(stats.messages_handled, 1);
    assert_eq!(stats.messages_failed, 0);
}

// ============================================================================
// Test 2: Service thread processes multiple commands
// ============================================================================

#[test]
fn service_thread_processes_multiple_commands() {
    let queue = InMemoryQueue::new();
    let repo = HashMapRepository::new();

    let service = DomainService::new(repo.clone())
        .command("counter.create", |repo, msg| {
            let data: CreateCounter = msg.decode()?;
            let mut counter = Counter::default();
            counter.create(data.id);
            repo.commit_aggregate(&mut counter)?;
            Ok(())
        })
        .command("counter.increment", |repo, msg| {
            let data: IncrementCounter = msg.decode()?;
            let mut counter: Counter = repo
                .get_aggregate(&data.id)?
                .ok_or_else(|| HandlerError::NotFound(data.id.clone()))?;
            counter.increment(data.amount);
            repo.commit_aggregate(&mut counter)?;
            Ok(())
        });

    let handle = DomainServiceThread::spawn(
        service,
        "counters".to_string(),
        queue.clone(),
        Duration::from_millis(10),
    );

    // Send create + increment commands
    queue
        .send(
            "counters",
            Message::encode("cmd-1", "counter.create", &CreateCounter { id: "c1".into() })
                .unwrap(),
        )
        .unwrap();
    queue
        .send(
            "counters",
            Message::encode(
                "cmd-2",
                "counter.increment",
                &IncrementCounter {
                    id: "c1".into(),
                    amount: 10,
                },
            )
            .unwrap(),
        )
        .unwrap();
    queue
        .send(
            "counters",
            Message::encode(
                "cmd-3",
                "counter.increment",
                &IncrementCounter {
                    id: "c1".into(),
                    amount: 5,
                },
            )
            .unwrap(),
        )
        .unwrap();

    // Wait for all messages to be processed
    thread::sleep(Duration::from_millis(300));

    // Verify final state: 0 + 10 + 5 = 15
    let counter: Counter = repo.get_aggregate("c1").unwrap().unwrap();
    assert_eq!(counter.value, 15);

    let stats = handle.stop();
    assert_eq!(stats.messages_handled, 3);
    assert_eq!(stats.messages_failed, 0);
}

// ============================================================================
// Test 3: Service thread tracks failed messages
// ============================================================================

#[test]
fn service_thread_tracks_failures() {
    let queue = InMemoryQueue::new();
    let repo = HashMapRepository::new();

    let service =
        DomainService::new(repo.clone()).command("counter.increment", |repo, msg| {
            let data: IncrementCounter = msg.decode()?;
            let mut counter: Counter = repo
                .get_aggregate(&data.id)?
                .ok_or_else(|| HandlerError::NotFound(data.id.clone()))?;
            counter.increment(data.amount);
            repo.commit_aggregate(&mut counter)?;
            Ok(())
        });

    let handle = DomainServiceThread::spawn(
        service,
        "counters".to_string(),
        queue.clone(),
        Duration::from_millis(10),
    );

    // Send increment for non-existent counter — will fail
    queue
        .send(
            "counters",
            Message::encode(
                "cmd-1",
                "counter.increment",
                &IncrementCounter {
                    id: "nonexistent".into(),
                    amount: 1,
                },
            )
            .unwrap(),
        )
        .unwrap();

    thread::sleep(Duration::from_millis(200));

    let stats = handle.stop();
    assert_eq!(stats.messages_handled, 0);
    assert_eq!(stats.messages_failed, 1);
}

// ============================================================================
// Test 4: Stop and stats
// ============================================================================

#[test]
fn service_thread_stop_returns_stats() {
    let queue = InMemoryQueue::new();
    let repo = HashMapRepository::new();

    let service = DomainService::new(repo).command("noop", |_, _| Ok(()));

    let handle = DomainServiceThread::spawn(
        service,
        "test-queue".to_string(),
        queue.clone(),
        Duration::from_millis(10),
    );

    // Let it poll a few times
    thread::sleep(Duration::from_millis(100));

    let stats = handle.stop();
    assert!(stats.polls > 0, "Should have polled at least once");
    assert_eq!(stats.messages_handled, 0);
    assert_eq!(stats.messages_failed, 0);
}

// ============================================================================
// Test 5: Multiple services on different queues
// ============================================================================

#[test]
fn multiple_services_on_different_queues() {
    let queue = InMemoryQueue::new();
    let repo = HashMapRepository::new();

    // Service A handles "counter.create" on "counters" queue
    let service_a = DomainService::new(repo.clone()).command("counter.create", |repo, msg| {
        let data: CreateCounter = msg.decode()?;
        let mut counter = Counter::default();
        counter.create(data.id);
        repo.commit_aggregate(&mut counter)?;
        Ok(())
    });

    // Service B handles "counter.increment" on "increments" queue
    let service_b =
        DomainService::new(repo.clone()).command("counter.increment", |repo, msg| {
            let data: IncrementCounter = msg.decode()?;
            let mut counter: Counter = repo
                .get_aggregate(&data.id)?
                .ok_or_else(|| HandlerError::NotFound(data.id.clone()))?;
            counter.increment(data.amount);
            repo.commit_aggregate(&mut counter)?;
            Ok(())
        });

    let handle_a = DomainServiceThread::spawn(
        service_a,
        "counters".to_string(),
        queue.clone(),
        Duration::from_millis(10),
    );

    let handle_b = DomainServiceThread::spawn(
        service_b,
        "increments".to_string(),
        queue.clone(),
        Duration::from_millis(10),
    );

    // Send create to "counters" queue
    queue
        .send(
            "counters",
            Message::encode("cmd-1", "counter.create", &CreateCounter { id: "c1".into() })
                .unwrap(),
        )
        .unwrap();

    thread::sleep(Duration::from_millis(200));

    // Send increment to "increments" queue
    queue
        .send(
            "increments",
            Message::encode(
                "cmd-2",
                "counter.increment",
                &IncrementCounter {
                    id: "c1".into(),
                    amount: 42,
                },
            )
            .unwrap(),
        )
        .unwrap();

    thread::sleep(Duration::from_millis(200));

    // Verify
    let counter: Counter = repo.get_aggregate("c1").unwrap().unwrap();
    assert_eq!(counter.value, 42);

    let stats_a = handle_a.stop();
    let stats_b = handle_b.stop();
    assert_eq!(stats_a.messages_handled, 1);
    assert_eq!(stats_b.messages_handled, 1);
}
