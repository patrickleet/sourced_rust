mod aggregate;

use aggregate::{Todo, TodoEvent};
use sourced_rust::{
    Aggregate, AggregateBuilder, EventRecord, HashMapRepository, Queueable,
};

#[test]
fn enum_variants_exist_and_compile() {
    let init = TodoEvent::Initialized {
        id: "1".into(),
        user_id: "alice".into(),
        task: "Buy milk".into(),
    };
    let completed = TodoEvent::Completed;

    // Debug and Clone derives work
    let _ = format!("{:?}", init);
    let _ = init.clone();
    let _ = format!("{:?}", completed);
}

#[test]
fn event_name_returns_correct_strings() {
    let init = TodoEvent::Initialized {
        id: "1".into(),
        user_id: "alice".into(),
        task: "Buy milk".into(),
    };
    assert_eq!(init.event_name(), "Initialized");

    let completed = TodoEvent::Completed;
    assert_eq!(completed.event_name(), "Completed");
}

#[test]
fn try_from_event_record_initialized() {
    let payload = bitcode::serialize(&(
        "t1".to_string(),
        "alice".to_string(),
        "Buy milk".to_string(),
    ))
    .unwrap();
    let record = EventRecord::new("Initialized", payload, 1);
    let event = TodoEvent::try_from(&record).unwrap();
    match event {
        TodoEvent::Initialized { id, user_id, task } => {
            assert_eq!(id, "t1");
            assert_eq!(user_id, "alice");
            assert_eq!(task, "Buy milk");
        }
        _ => panic!("Expected Initialized variant"),
    }
}

#[test]
fn try_from_event_record_completed() {
    let record = EventRecord::new("Completed", vec![], 2);
    let event = TodoEvent::try_from(&record).unwrap();
    assert_eq!(event, TodoEvent::Completed);
}

#[test]
fn try_from_unknown_event_returns_error() {
    let record = EventRecord::new("Unknown", vec![], 1);
    let result = TodoEvent::try_from(&record);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Unknown event"));
}

#[test]
fn aggregate_hydration_roundtrip() {
    let repo = HashMapRepository::new().queued().aggregate::<Todo>();

    let mut todo = Todo::default();
    todo.initialize("t1".into(), "alice".into(), "Buy milk".into());
    todo.complete();

    repo.commit(&mut todo).unwrap();

    let loaded = repo.get("t1").unwrap().unwrap();
    assert_eq!(loaded.snapshot().id, "t1");
    assert_eq!(loaded.snapshot().user_id, "alice");
    assert_eq!(loaded.snapshot().task, "Buy milk");
    assert!(loaded.snapshot().completed);
}

#[test]
fn guard_condition_works() {
    let mut todo = Todo::default();
    todo.initialize("t1".into(), "alice".into(), "Test".into());
    todo.complete();
    // Second complete should be no-op (guard: !self.completed)
    todo.complete();

    assert_eq!(todo.entity.version(), 2); // only Initialized + Completed
}

#[test]
fn non_event_methods_pass_through() {
    let mut todo = Todo::default();
    todo.initialize("t1".into(), "alice".into(), "Test".into());
    let snap = todo.snapshot();
    assert_eq!(snap.id, "t1");
    assert_eq!(snap.user_id, "alice");
}

#[test]
fn partial_equality_on_enum() {
    let a = TodoEvent::Initialized {
        id: "1".into(),
        user_id: "alice".into(),
        task: "Test".into(),
    };
    let b = TodoEvent::Initialized {
        id: "1".into(),
        user_id: "alice".into(),
        task: "Test".into(),
    };
    let c = TodoEvent::Completed;

    assert_eq!(a, b);
    assert_ne!(a, c);
}

#[test]
fn no_upcasters_by_default() {
    assert!(Todo::upcasters().is_empty());
}
