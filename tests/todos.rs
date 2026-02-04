mod support;

use bitcode;
use sourced_rust::{
    AggregateBuilder, Commit, EventEmitter, GetAggregate, HashMapRepository, LocalEmitterPublisher,
    LogPublisher, OutboxCommitExt, OutboxMessage, OutboxRepositoryExt, OutboxWorker,
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;
use support::todo::{Todo, TodoRepository, TodoSnapshot};

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
    todo.initialize(id1.clone(), "user1".to_string(), "Buy groceries".to_string());

    // Add an outbox event for the initialization
    let mut init_message = OutboxMessage::encode(
        format!("{}:init", id1),
        "TodoInitialized",
        &todo.snapshot(),
    )
    .unwrap();

    // Commit the Todo + Outbox message to the repository
    let _ = repo.outbox(&mut init_message).commit(&mut todo);

    // Verify the outbox event was captured
    {
        let pending = repo.outbox_messages_pending().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].event_type, "TodoInitialized");
    }

    // Retrieve the Todo from the repository and complete it, then commit again
    if let Some(mut retrieved_todo) = repo.get(&id1).unwrap() {
        retrieved_todo.complete();

        // Add an outbox event for the completion
        let mut complete_message = OutboxMessage::encode(
            format!("{}:complete", id1),
            "TodoCompleted",
            &retrieved_todo.snapshot(),
        )
        .unwrap();

        let _ = repo
            .outbox(&mut complete_message)
            .commit(&mut retrieved_todo);

        // Verify we now have 2 outbox events
        {
            let pending = repo.outbox_messages_pending().unwrap();
            assert_eq!(pending.len(), 2);
            assert!(pending
                .iter()
                .any(|msg| msg.event_type == "TodoInitialized"));
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
    todo3.initialize(id3.clone(), "user2".to_string(), "Chew bubblegum".to_string());

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

    let todos = repo.get_all(&[&id1, &id2]).unwrap();
    assert_eq!(todos.len(), 2);
    assert_eq!(todos[0].snapshot().id, id1);
    assert_eq!(todos[0].snapshot().completed, false);
    assert_eq!(todos[1].snapshot().id, id2);
    assert_eq!(todos[1].snapshot().completed, false);

    let mut iter = todos.into_iter();
    let mut todo1v2 = iter.next().unwrap();
    let mut todo2v2 = iter.next().unwrap();

    todo1v2.complete();
    todo2v2.complete();

    repo.commit_all(&mut [&mut todo1v2, &mut todo2v2]).unwrap();

    let v2_todos = repo.peek_all(&[&id1, &id2]).unwrap();

    assert_eq!(v2_todos.len(), 2);
    assert_eq!(v2_todos[0].snapshot().id, id1);
    assert_eq!(v2_todos[0].snapshot().completed, true);
    assert_eq!(v2_todos[1].snapshot().id, id2);
    assert_eq!(v2_todos[1].snapshot().completed, true);
}

#[test]
fn outbox_records_persisted() {
    let repo = HashMapRepository::new();
    let mut todo = Todo::new();
    let id = next_id();
    todo.initialize(id.clone(), "user1".to_string(), "Outbox demo".to_string());
    let snapshot = todo.snapshot();
    let mut message = OutboxMessage::encode(
        format!("{}:init", id),
        "TodoInitialized",
        &snapshot,
    )
    .unwrap();

    repo.commit(&mut [&mut todo.entity, &mut message.entity])
        .unwrap();

    // Check pending outbox messages
    let pending = repo.outbox_messages_pending().unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].event_type, "TodoInitialized");

    let published: TodoSnapshot = bitcode::deserialize(&pending[0].payload).unwrap();
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
    let mut message = OutboxMessage::encode(
        format!("{}:init", id),
        "TodoInitialized",
        &snapshot,
    )
    .unwrap();
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
        .claim_outbox_messages("logger-1", 10, Duration::from_secs(30))
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
        .get_aggregate::<OutboxMessage>(&message_id)
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
    let mut message = OutboxMessage::encode(
        format!("{}:init", id),
        "TodoInitialized",
        &snapshot,
    )
    .unwrap();
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
        .claim_outbox_messages("emitter-1", 10, Duration::from_secs(30))
        .unwrap();
    let result = worker.process_batch(&mut claimed);
    assert_eq!(result.completed, 1);
    for message in &mut claimed {
        repo.commit(&mut message.entity).unwrap();
    }

    // LocalEmitterPublisher converts bytes to lossy string, so we just verify something was received
    let payload = rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(!payload.is_empty());
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
    assert!(rx_got.recv_timeout(Duration::from_millis(200)).is_err());

    repo.abort(&locked).unwrap();
    assert!(rx_got.recv_timeout(Duration::from_millis(500)).is_ok());
}

#[test]
fn abort_releases_lock_after_get_all() {
    let repo = Arc::new(TodoRepository::new());
    let mut todo1 = Todo::new();
    let id1 = next_id();
    todo1.initialize(
        id1.clone(),
        "user1".to_string(),
        "Abort get_all 1".to_string(),
    );
    repo.commit(&mut todo1).unwrap();

    let mut todo2 = Todo::new();
    let id2 = next_id();
    todo2.initialize(
        id2.clone(),
        "user2".to_string(),
        "Abort get_all 2".to_string(),
    );
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
    assert!(rx_got.recv_timeout(Duration::from_millis(200)).is_err());

    for todo in &locked {
        repo.abort(todo).unwrap();
    }

    assert!(rx_got.recv_timeout(Duration::from_millis(500)).is_ok());
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
    let mut message1 = OutboxMessage::encode(format!("{}:1", id), "Event1", &snapshot).unwrap();
    let mut message2 = OutboxMessage::encode(format!("{}:2", id), "Event2", &snapshot).unwrap();
    let mut message3 = OutboxMessage::encode(format!("{}:3", id), "Event3", &snapshot).unwrap();

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
            .claim_outbox_messages("safe-worker", 1, Duration::from_secs(30))
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
        let message = repo.get_aggregate::<OutboxMessage>(id).unwrap().unwrap();
        assert!(message.is_published());
    }
    assert_eq!(repo.outbox_messages_pending().unwrap().len(), 0);

    let lines = buffer.lock().unwrap();
    assert_eq!(lines.len(), 3);
}

#[test]
fn find_returns_matching_aggregates() {
    // Use HashMapRepository directly (no queuing) for read-only find tests
    let repo = HashMapRepository::new().aggregate::<Todo>();

    // Create todos for different users
    let mut todo1 = Todo::new();
    let id1 = next_id();
    todo1.initialize(
        id1.clone(),
        "alice".to_string(),
        "Buy groceries".to_string(),
    );

    let mut todo2 = Todo::new();
    let id2 = next_id();
    todo2.initialize(id2.clone(), "alice".to_string(), "Walk the dog".to_string());

    let mut todo3 = Todo::new();
    let id3 = next_id();
    todo3.initialize(id3.clone(), "bob".to_string(), "Write code".to_string());

    repo.commit_all(&mut [&mut todo1, &mut todo2, &mut todo3])
        .unwrap();

    // Find all todos for alice
    let alice_todos = repo.find(|t| t.snapshot().user_id == "alice").unwrap();
    assert_eq!(alice_todos.len(), 2);

    // Find all todos for bob
    let bob_todos = repo.find(|t| t.snapshot().user_id == "bob").unwrap();
    assert_eq!(bob_todos.len(), 1);
    assert_eq!(bob_todos[0].snapshot().task, "Write code");

    // Find with no matches
    let charlie_todos = repo.find(|t| t.snapshot().user_id == "charlie").unwrap();
    assert!(charlie_todos.is_empty());
}

#[test]
fn find_one_returns_first_matching_aggregate() {
    let repo = HashMapRepository::new().aggregate::<Todo>();

    let mut todo1 = Todo::new();
    let id1 = next_id();
    todo1.initialize(id1.clone(), "alice".to_string(), "First task".to_string());

    let mut todo2 = Todo::new();
    let id2 = next_id();
    todo2.initialize(id2.clone(), "alice".to_string(), "Second task".to_string());

    repo.commit_all(&mut [&mut todo1, &mut todo2]).unwrap();

    // Find one for alice
    let found = repo.find_one(|t| t.snapshot().user_id == "alice").unwrap();
    assert!(found.is_some());
    let todo = found.unwrap();
    assert_eq!(todo.snapshot().user_id, "alice");

    // Find one with no match
    let not_found = repo.find_one(|t| t.snapshot().user_id == "nobody").unwrap();
    assert!(not_found.is_none());
}

#[test]
fn exists_returns_true_when_aggregate_matches() {
    let repo = HashMapRepository::new().aggregate::<Todo>();

    let mut todo = Todo::new();
    let id = next_id();
    todo.initialize(id.clone(), "alice".to_string(), "Test task".to_string());
    repo.commit(&mut todo).unwrap();

    assert!(repo.exists(|t| t.snapshot().user_id == "alice").unwrap());
    assert!(!repo.exists(|t| t.snapshot().user_id == "bob").unwrap());
}

#[test]
fn count_returns_matching_aggregate_count() {
    let repo = HashMapRepository::new().aggregate::<Todo>();

    let mut todo1 = Todo::new();
    let id1 = next_id();
    todo1.initialize(id1.clone(), "alice".to_string(), "Task 1".to_string());

    let mut todo2 = Todo::new();
    let id2 = next_id();
    todo2.initialize(id2.clone(), "alice".to_string(), "Task 2".to_string());

    let mut todo3 = Todo::new();
    let id3 = next_id();
    todo3.initialize(id3.clone(), "bob".to_string(), "Task 3".to_string());

    repo.commit_all(&mut [&mut todo1, &mut todo2, &mut todo3])
        .unwrap();

    assert_eq!(repo.count(|t| t.snapshot().user_id == "alice").unwrap(), 2);
    assert_eq!(repo.count(|t| t.snapshot().user_id == "bob").unwrap(), 1);
    assert_eq!(repo.count(|_| true).unwrap(), 3);
    assert_eq!(
        repo.count(|t| t.snapshot().user_id == "charlie").unwrap(),
        0
    );
}

#[test]
fn find_by_completed_status() {
    let repo = HashMapRepository::new().aggregate::<Todo>();

    let mut todo1 = Todo::new();
    let id1 = next_id();
    todo1.initialize(
        id1.clone(),
        "alice".to_string(),
        "Completed task".to_string(),
    );
    todo1.complete();

    let mut todo2 = Todo::new();
    let id2 = next_id();
    todo2.initialize(id2.clone(), "alice".to_string(), "Pending task".to_string());

    repo.commit_all(&mut [&mut todo1, &mut todo2]).unwrap();

    // Find completed todos
    let completed = repo.find(|t| t.snapshot().completed).unwrap();
    assert_eq!(completed.len(), 1);
    assert_eq!(completed[0].snapshot().task, "Completed task");

    // Find pending todos
    let pending = repo.find(|t| !t.snapshot().completed).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].snapshot().task, "Pending task");
}
