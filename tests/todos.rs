mod support;

use serde_json;
use sourced_rust::{
    EventEmitter, HashMapRepository, LocalEmitterPublisher, LogPublisher, OutboxDelivery,
    OutboxDeliveryResult, OutboxRepository, OutboxStatus, OutboxWorker, Repository,
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

    // Create a new Todo
    let mut todo = Todo::new();

    // Add event listeners
    let id1 = next_id();
    let id1_for_init = id1.clone();
    todo.entity.on("ToDoInitialized", move |data| {
        match Todo::deserialize(&data) {
            Ok(deserialized_todo) => {
                assert!(deserialized_todo.snapshot().id == id1_for_init);
                assert!(deserialized_todo.snapshot().user_id == "user1");
                assert!(deserialized_todo.snapshot().task == "Buy groceries");
                assert!(!deserialized_todo.snapshot().completed);
            }
            Err(e) => {
                println!("Error deserializing Todo: {}", e);
            }
        }
    });

    todo.initialize(
        id1.clone(),
        "user1".to_string(),
        "Buy groceries".to_string(),
    );

    // Commit the Todo to the repository
    let _ = repo.commit(&mut todo);

    // Retrieve the Todo from the repository and complete it, then commit again
    if let Some(mut retrieved_todo) = repo.get(&id1).unwrap() {
        let id1_for_complete = id1.clone();
        retrieved_todo.entity.on("ToDoCompleted", move |data| {
            match Todo::deserialize(&data) {
                Ok(deserialized_todo) => {
                    assert!(deserialized_todo.snapshot().id == id1_for_complete);
                    assert!(deserialized_todo.snapshot().user_id == "user1");
                    assert!(deserialized_todo.snapshot().task == "Buy groceries");
                    assert!(deserialized_todo.snapshot().completed);
                }
                Err(e) => {
                    println!("Error deserializing Todo: {}", e);
                }
            }
        });

        retrieved_todo.complete();

        let _ = repo.commit(&mut retrieved_todo);

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
    let all_todos = repo.get_all(&[&id1, &id2, &id3]).unwrap();
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

    let retrieved = repo.get(&id).unwrap().expect("Todo not found");
    assert_eq!(retrieved.snapshot().id, id);
    assert_eq!(retrieved.snapshot().user_id, "user1");
    assert_eq!(retrieved.snapshot().task, "Roundtrip");
    assert!(!retrieved.snapshot().completed);

    repo.abort(&retrieved).unwrap();
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
    assert_eq!(todos[1].snapshot().id, id2);

    for todo in &todos {
        repo.abort(todo).unwrap();
    }
}

#[test]
fn outbox_records_persisted() {
    let repo = HashMapRepository::new();
    let mut todo = Todo::new();
    let id = next_id();
    todo.initialize(id.clone(), "user1".to_string(), "Outbox demo".to_string());
    let snapshot = todo.snapshot();
    todo.entity
        .outbox("TodoInitialized", serde_json::to_string(&snapshot).unwrap());

    repo.commit(&mut todo.entity).unwrap();

    let outbox = repo
        .claim_outbox("worker-1", 10, Duration::from_secs(30))
        .unwrap();
    assert_eq!(outbox.len(), 1);
    assert_eq!(outbox[0].aggregate_id, id);
    assert_eq!(outbox[0].event_type, "TodoInitialized");
    assert_eq!(outbox[0].status, OutboxStatus::InFlight);
    let published: TodoSnapshot = serde_json::from_str(&outbox[0].payload).unwrap();
    assert_eq!(published.id, snapshot.id);
    assert_eq!(published.user_id, snapshot.user_id);
    assert_eq!(published.task, snapshot.task);
    assert_eq!(published.completed, snapshot.completed);

    repo.complete_outbox(&[outbox[0].id]).unwrap();
    let remaining = repo.peek_outbox().unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].status, OutboxStatus::Published);
}

#[test]
fn outbox_delivery_helpers() {
    let repo = HashMapRepository::new();
    let mut todo = Todo::new();
    let id = next_id();
    todo.initialize(
        id.clone(),
        "user1".to_string(),
        "Outbox retries".to_string(),
    );
    let snapshot = todo.snapshot();
    todo.entity
        .outbox("TodoInitialized", serde_json::to_string(&snapshot).unwrap());
    repo.commit(&mut todo.entity).unwrap();

    let result = repo
        .deliver_outbox("worker-1", 10, Duration::from_secs(30), 1, |_record| {
            Err::<(), _>("boom")
        })
        .unwrap();

    assert_eq!(
        result,
        OutboxDeliveryResult {
            claimed: 1,
            completed: 0,
            released: 0,
            failed: 1,
        }
    );

    let outbox = repo.peek_outbox().unwrap();
    assert_eq!(outbox.len(), 1);
    assert_eq!(outbox[0].status, OutboxStatus::Failed);
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
    todo.entity
        .outbox("TodoInitialized", serde_json::to_string(&snapshot).unwrap());
    repo.commit(&mut todo.entity).unwrap();

    let buffer = Arc::new(Mutex::new(Vec::new()));
    let publisher = LogPublisher::with_buffer(Arc::clone(&buffer));
    let mut worker = OutboxWorker::new(repo, publisher);

    let result = worker
        .drain_once("logger-1", 10, Duration::from_secs(30), 3)
        .unwrap();
    assert_eq!(result.completed, 1);

    let lines = buffer.lock().unwrap();
    assert_eq!(lines.len(), 1);
    assert!(lines[0].contains("TodoInitialized"));
    assert!(lines[0].contains(&snapshot.id));

    let outbox = worker.repo().peek_outbox().unwrap();
    assert_eq!(outbox.len(), 1);
    assert_eq!(outbox[0].status, OutboxStatus::Published);
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
    todo.entity
        .outbox("TodoInitialized", serde_json::to_string(&snapshot).unwrap());
    repo.commit(&mut todo.entity).unwrap();

    let mut emitter = EventEmitter::new();
    let (tx, rx) = mpsc::channel::<String>();
    emitter.on("TodoInitialized", move |payload: String| {
        tx.send(payload).unwrap();
    });

    let publisher = LocalEmitterPublisher::new(emitter);
    let mut worker = OutboxWorker::new(repo, publisher);

    let result = worker
        .drain_once("emitter-1", 10, Duration::from_secs(30), 3)
        .unwrap();
    assert_eq!(result.completed, 1);

    let payload = rx.recv_timeout(Duration::from_secs(1)).unwrap();
    let published: TodoSnapshot = serde_json::from_str(&payload).unwrap();
    assert_eq!(published.id, snapshot.id);
    assert_eq!(published.user_id, snapshot.user_id);
    assert_eq!(published.task, snapshot.task);
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
