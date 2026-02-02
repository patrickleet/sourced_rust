mod support;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::Duration;
use support::todo::Todo;
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
    let id1 = next_id();
    todo.initialize(
        id1.clone(),
        "user1".to_string(),
        "Buy groceries".to_string(),
    );

    // Add event listeners
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

    // Commit the Todo to the repository
    let _ = repo.commit(&mut todo);

    // Retrieve the Todo from the repository
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

        // Complete the Todo
        retrieved_todo.complete();

        // Commit the changes
        let _ = repo.commit(&mut retrieved_todo);

        // Retrieve the Todo again to demonstrate that events are fired on retrieval
        if let Some(mut updated_todo) = repo.get(&id1).unwrap() {
            assert!(updated_todo.snapshot().id == id1);
            assert!(updated_todo.snapshot().user_id == "user1");
            assert!(updated_todo.snapshot().task == "Buy groceries");
            assert!(updated_todo.snapshot().completed);
            let _ = repo.commit(&mut updated_todo);
        } else {
            println!("Updated Todo not found");
        }
    } else {
        println!("Todo not found");
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
fn queued_repo_blocks_get_until_commit() {
    let repo = Arc::new(TodoRepository::new());
    let mut todo = Todo::new();
    let id = next_id();
    todo.initialize(id.clone(), "user1".to_string(), "Queue test".to_string());
    repo.commit(&mut todo).unwrap();

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
