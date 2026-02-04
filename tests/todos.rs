mod support;

use serde_json;
use sourced_rust::{
    DomainEvent, EventEmitter, HashMapRepository, LocalEmitterPublisher, LogPublisher,
    OutboxWorker, Repository, RepositoryExt,
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;
use support::todo::{Todo, TodoSnapshot};
use support::todo_repository::TodoRepository;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn next_id() -> String {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    format!("todo-{}", id)
}

#[test]
fn todos() {
    let repo = TodoRepository::new();

    // Create a new Todo + Outbox messages
    let mut todo = Todo::new();
    let id1 = next_id();

    todo.initialize(
        id1.clone(),
        "user1".to_string(),
        "Buy groceries".to_string(),
    );

    // Add an outbox event for the initialization
    let mut init_message = DomainEvent::new(
        format!("{}:init", id1),
        "TodoInitialized",
        serde_json::to_string(&todo.snapshot()).unwrap(),
    );

    // Commit the Todo + Outbox message to the repository
    let _ = repo
        .repo()
        .commit(&mut [&mut todo.entity, &mut init_message.entity]);

    // Verify the outbox event was captured
    {
        let pending = repo.repo().inner().domain_events_pending().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].event_type, "TodoInitialized");
    }

    // Retrieve the Todo from the repository and complete it, then commit again
    if let Some(mut retrieved_todo) = repo.get(&id1).unwrap() {
        retrieved_todo.complete();

        // Add an outbox event for the completion
        let mut complete_message = DomainEvent::new(
            format!("{}:complete", id1),
            "TodoCompleted",
            serde_json::to_string(&retrieved_todo.snapshot()).unwrap(),
        );

        let _ = repo
            .repo()
            .commit(&mut [&mut retrieved_todo.entity, &mut complete_message.entity]);

        // Verify we now have 2 outbox events
        {
            let pending = repo.repo().inner().domain_events_pending().unwrap();
            assert_eq!(pending.len(), 2);
            assert!(pending.iter().any(|msg| msg.event_type == "TodoInitialized"));
            assert!(pending.iter().any(|msg| msg.event_type == "TodoCompleted"));
        }

        if let Some(completed_todo) = repo.get(&id1).unwrap() {
            assert!(completed_todo.snapshot().id == id1);
            assert!(completed_todo.snapshot().user_id == "user1");
            assert!(completed_todo.snapshot().task == "Buy groceries");
            assert!(completed_todo.snapshot().completed);

            repo.abort(&completed_todo).unwrap();
        } else {
            panic!("Updated Todo not found");
        }
    } else {
        panic!("Todo not found");
    }

    let mut todo2 = Todo::new();
    let id2 = next_id();
    todo2.initialize(id2.clone(), "user1".to_string(), "Buy Sauna".to_string());

    let mut todo3 = Todo::new();
    let id3 = next_id();
    todo3.initialize(
        id3.clone(),
        "user2".to_string(),
        "Chew bubblegum".to_string(),
    );

    // Commit multiple Todos to the repository
    let _ = repo.commit_all(&mut [&mut todo2, &mut todo3]);

    // get all the todos from the repository
    let all_todos = repo.peek_all(&[&id1, &id2, &id3]).unwrap();
    if !all_todos.is_empty() {
        assert!(all_todos.len() == 3);
    } else {
        println!("No Todos found");
    }
}

#[test]
fn get_commit_roundtrip() {
    let repo = TodoRepository::new();
    let mut todo = Todo::new();
    let id = next_id();
    todo.initialize(id.clone(), "user1".to_string(), "Roundtrip".to_string());

    repo.commit(&mut todo).unwrap();

    let retrieved = repo.peek(&id).unwrap().expect("Todo not found");
    assert_eq!(retrieved.snapshot().id, id);
    assert_eq!(retrieved.snapshot().user_id, "user1");
    assert_eq!(retrieved.snapshot().task, "Roundtrip");
    assert!(!retrieved.snapshot().completed);
}

#[test]
fn get_all_commit_all_roundtrip() {
    let repo = TodoRepository::new();

    let mut todo1 = Todo::new();
    let id1 = next_id();
    todo1.initialize(id1.clone(), "user1".to_string(), "First".to_string());

    let mut todo2 = Todo::new();
    let id2 = next_id();
    todo2.initialize(id2.clone(), "user2".to_string(), "Second".to_string());

    repo.commit_all(&mut [&mut todo1, &mut todo2]).unwrap();

    let todos = repo.peek_all(&[&id1, &id2]).unwrap();
    assert_eq!(todos.len(), 2);
    assert_eq!(todos[0].snapshot().id, id1);
    assert_eq!(todos[1].snapshot().id, id2);
}

#[test]
fn outbox_records_persisted() {
    let repo = HashMapRepository::new();
    let mut todo = Todo::new();
    let id = next_id();
    todo.initialize(id.clone(), "user1".to_string(), "Outbox demo".to_string());
    let snapshot = todo.snapshot();
    let mut message = DomainEvent::new(
        format!("{}:init", id),
        "TodoInitialized",
        serde_json::to_string(&snapshot).unwrap(),
    );

    repo.commit(&mut [&mut todo.entity, &mut message.entity])
        .unwrap();

    // Check pending outbox messages
    let pending = repo.domain_events_pending().unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].event_type, "TodoInitialized");

    let published: TodoSnapshot = serde_json::from_str(&pending[0].payload).unwrap();
    assert_eq!(published.id, snapshot.id);
    assert_eq!(published.user_id, snapshot.user_id);
    assert_eq!(published.task, snapshot.task);
    assert_eq!(published.completed, snapshot.completed);
}

#[test]
fn outbox_worker_log_publisher() {
    let repo = HashMapRepository::new();
    let mut todo = Todo::new();
    let id = next_id();
    todo.initialize(
        id.clone(),
        "user1".to_string(),
        "Outbox log publisher".to_string(),
    );
    let snapshot = todo.snapshot();
    let mut message = DomainEvent::new(
        format!("{}:init", id),
        "TodoInitialized",
        serde_json::to_string(&snapshot).unwrap(),
    );
    let message_id = message.id().to_string();
    repo.commit(&mut [&mut todo.entity, &mut message.entity])
        .unwrap();

    // Create worker with new API
    let buffer = Arc::new(Mutex::new(Vec::new()));
    let publisher = LogPublisher::with_buffer(Arc::clone(&buffer));
    let mut worker = OutboxWorker::new(publisher)
        .with_worker_id("logger-1")
        .with_batch_size(10)
        .with_max_attempts(3);

    // Claim pending messages and process
    let mut claimed = repo
        .claim_domain_events("logger-1", 10, Duration::from_secs(30))
        .unwrap();
    let result = worker.process_batch(&mut claimed);
    assert_eq!(result.completed, 1);
    for message in &mut claimed {
        repo.commit(&mut message.entity).unwrap();
    }

    let lines = buffer.lock().unwrap();
    assert_eq!(lines.len(), 1);
    assert!(lines[0].contains("TodoInitialized"));

    // Check record is marked as published
    let published = repo
        .get_aggregate::<DomainEvent>(&message_id)
        .unwrap()
        .unwrap();
    assert!(published.is_published());
}

#[test]
fn outbox_worker_local_emitter_publisher() {
    let repo = HashMapRepository::new();
    let mut todo = Todo::new();
    let id = next_id();
    todo.initialize(
        id.clone(),
        "user1".to_string(),
        "Outbox local emitter".to_string(),
    );
    let snapshot = todo.snapshot();
    let mut message = DomainEvent::new(
        format!("{}:init", id),
        "TodoInitialized",
        serde_json::to_string(&snapshot).unwrap(),
    );
    repo.commit(&mut [&mut todo.entity, &mut message.entity])
        .unwrap();

    let mut emitter = EventEmitter::new();
    let (tx, rx) = mpsc::channel::<String>();
    emitter.on("TodoInitialized", move |payload: String| {
        tx.send(payload).unwrap();
    });

    let publisher = LocalEmitterPublisher::new(emitter);
    let mut worker = OutboxWorker::new(publisher)
        .with_worker_id("emitter-1")
        .with_batch_size(10)
        .with_max_attempts(3);

    // Claim pending messages and process
    let mut claimed = repo
        .claim_domain_events("emitter-1", 10, Duration::from_secs(30))
        .unwrap();
    let result = worker.process_batch(&mut claimed);
    assert_eq!(result.completed, 1);
    for message in &mut claimed {
        repo.commit(&mut message.entity).unwrap();
    }

    let payload = rx.recv_timeout(Duration::from_secs(1)).unwrap();
    let published: TodoSnapshot = serde_json::from_str(&payload).unwrap();
    assert_eq!(published.id, snapshot.id);
    assert_eq!(published.user_id, snapshot.user_id);
    assert_eq!(published.task, snapshot.task);
}

#[test]
fn abort_releases_lock_after_get() {
    let repo = Arc::new(TodoRepository::new());
    let mut todo = Todo::new();
    let id = next_id();
    todo.initialize(id.clone(), "user1".to_string(), "Abort get".to_string());
    repo.commit(&mut todo).unwrap();

    let locked = repo.get(&id).unwrap().unwrap();

    let (tx_started, rx_started) = mpsc::channel();
    let (tx_got, rx_got) = mpsc::channel();
    let repo_other = Arc::clone(&repo);
    let id_other = id.clone();
    thread::spawn(move || {
        tx_started.send(()).unwrap();
        let _ = repo_other.get(&id_other).unwrap();
        tx_got.send(()).unwrap();
    });

    rx_started.recv().unwrap();
    assert!(rx_got
        .recv_timeout(Duration::from_millis(200))
        .is_err());

    repo.abort(&locked).unwrap();
    assert!(rx_got
        .recv_timeout(Duration::from_millis(500))
        .is_ok());
}

#[test]
fn abort_releases_lock_after_get_all() {
    let repo = Arc::new(TodoRepository::new());
    let mut todo1 = Todo::new();
    let id1 = next_id();
    todo1.initialize(id1.clone(), "user1".to_string(), "Abort get_all 1".to_string());
    repo.commit(&mut todo1).unwrap();

    let mut todo2 = Todo::new();
    let id2 = next_id();
    todo2.initialize(id2.clone(), "user2".to_string(), "Abort get_all 2".to_string());
    repo.commit(&mut todo2).unwrap();

    let locked = repo.get_all(&[&id1, &id2]).unwrap();

    let (tx_started, rx_started) = mpsc::channel();
    let (tx_got, rx_got) = mpsc::channel();
    let repo_other = Arc::clone(&repo);
    let id_other = id1.clone();
    thread::spawn(move || {
        tx_started.send(()).unwrap();
        let _ = repo_other.get(&id_other).unwrap();
        tx_got.send(()).unwrap();
    });

    rx_started.recv().unwrap();
    assert!(rx_got
        .recv_timeout(Duration::from_millis(200))
        .is_err());

    for todo in &locked {
        repo.abort(todo).unwrap();
    }

    assert!(rx_got
        .recv_timeout(Duration::from_millis(500))
        .is_ok());
}

#[test]
fn queued_repo_blocks_get_until_commit() {
    let repo = Arc::new(TodoRepository::new());
    let mut todo = Todo::new();
    let id = next_id();
    todo.initialize(id.clone(), "user1".to_string(), "Queue test".to_string());
    repo.commit(&mut todo).unwrap();

    let mut other_todo = Todo::new();
    let other_id = next_id();
    other_todo.initialize(
        other_id.clone(),
        "user2".to_string(),
        "Independent queue".to_string(),
    );
    repo.commit(&mut other_todo).unwrap();

    let (tx_started, rx_started) = mpsc::channel();
    let (tx_release, rx_release) = mpsc::channel();
    let (tx_committed, rx_committed) = mpsc::channel();

    let repo_a = Arc::clone(&repo);
    let id_a = id.clone();
    thread::spawn(move || {
        let mut todo = repo_a.get(&id_a).unwrap().unwrap();
        tx_started.send(()).unwrap();
        rx_release.recv().unwrap();
        let _ = repo_a.commit(&mut todo);
        tx_committed.send(()).unwrap();
    });

    rx_started.recv().unwrap();

    let (tx_other_done, rx_other_done) = mpsc::channel();
    let repo_other = Arc::clone(&repo);
    let other_id_clone = other_id.clone();
    thread::spawn(move || {
        let todo = repo_other.get(&other_id_clone).unwrap().unwrap();
        repo_other.abort(&todo).unwrap();
        tx_other_done.send(()).unwrap();
    });

    let (tx_peek_done, rx_peek_done) = mpsc::channel();
    let repo_peek = Arc::clone(&repo);
    let id_peek = id.clone();
    thread::spawn(move || {
        let _ = repo_peek.peek(&id_peek).unwrap();
        tx_peek_done.send(()).unwrap();
    });

    let (tx_peek_all_done, rx_peek_all_done) = mpsc::channel();
    let repo_peek_all = Arc::clone(&repo);
    let id_peek_all = id.clone();
    thread::spawn(move || {
        let ids = [id_peek_all.as_str()];
        let _ = repo_peek_all.peek_all(&ids).unwrap();
        tx_peek_all_done.send(()).unwrap();
    });

    assert!(rx_peek_done
        .recv_timeout(Duration::from_millis(200))
        .is_ok());
    assert!(rx_peek_all_done
        .recv_timeout(Duration::from_millis(200))
        .is_ok());
    assert!(rx_other_done
        .recv_timeout(Duration::from_millis(200))
        .is_ok());

    let (tx_done, rx_done) = mpsc::channel();
    let repo_b = Arc::clone(&repo);
    let id_b = id.clone();
    thread::spawn(move || {
        let mut todo = repo_b.get(&id_b).unwrap().unwrap();
        let _ = repo_b.commit(&mut todo);
        tx_done.send(()).unwrap();
    });

    assert!(rx_done.recv_timeout(Duration::from_millis(200)).is_err());
    tx_release.send(()).unwrap();
    rx_committed.recv().unwrap();
    assert!(rx_done.recv_timeout(Duration::from_millis(500)).is_ok());
}

#[test]
fn outbox_worker_process_next_with_commit() {
    let repo = HashMapRepository::new();
    let mut todo = Todo::new();
    let id = next_id();
    todo.initialize(
        id.clone(),
        "user1".to_string(),
        "Process next test".to_string(),
    );
    let snapshot = todo.snapshot();

    // Queue 3 messages
    let mut message1 =
        DomainEvent::new(format!("{}:1", id), "Event1", serde_json::to_string(&snapshot).unwrap());
    let mut message2 =
        DomainEvent::new(format!("{}:2", id), "Event2", serde_json::to_string(&snapshot).unwrap());
    let mut message3 =
        DomainEvent::new(format!("{}:3", id), "Event3", serde_json::to_string(&snapshot).unwrap());

    let message_ids = vec![
        message1.id().to_string(),
        message2.id().to_string(),
        message3.id().to_string(),
    ];

    repo.commit(&mut [
        &mut todo.entity,
        &mut message1.entity,
        &mut message2.entity,
        &mut message3.entity,
    ])
    .unwrap();

    let buffer = Arc::new(Mutex::new(Vec::new()));
    let publisher = LogPublisher::with_buffer(Arc::clone(&buffer));
    let mut worker = OutboxWorker::new(publisher)
        .with_worker_id("safe-worker")
        .with_batch_size(10)
        .with_max_attempts(3);

    // Process one at a time with commits
    let mut processed = 0;

    loop {
        let mut claimed = repo
            .claim_domain_events("safe-worker", 1, Duration::from_secs(30))
            .unwrap();
        if claimed.is_empty() {
            break;
        }
        let result = worker.process_batch(&mut claimed);
        processed += result.completed + result.released + result.failed;
        for message in &mut claimed {
            repo.commit(&mut message.entity).unwrap();
        }
    }

    assert_eq!(processed, 3);
    for id in &message_ids {
        let message = repo.get_aggregate::<DomainEvent>(id).unwrap().unwrap();
        assert!(message.is_published());
    }
    assert_eq!(repo.domain_events_pending().unwrap().len(), 0);

    let lines = buffer.lock().unwrap();
    assert_eq!(lines.len(), 3);
}
