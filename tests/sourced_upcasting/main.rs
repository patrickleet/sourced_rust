mod aggregate;

use aggregate::{TodoV1, TodoV1Event, TodoV2, TodoV2Event, TodoV3};
use sourced_rust::{hydrate, Aggregate, AggregateBuilder, Commit, Entity, HashMapRepository};

#[test]
fn v1_has_no_upcasters() {
    assert!(TodoV1::upcasters().is_empty());
}

#[test]
fn v2_has_one_upcaster() {
    let upcasters = TodoV2::upcasters();
    assert_eq!(upcasters.len(), 1);
    assert_eq!(upcasters[0].event_type, "Initialized");
    assert_eq!(upcasters[0].from_version, 1);
    assert_eq!(upcasters[0].to_version, 2);
}

#[test]
fn v3_has_two_upcasters() {
    let upcasters = TodoV3::upcasters();
    assert_eq!(upcasters.len(), 2);
    assert_eq!(upcasters[0].from_version, 1);
    assert_eq!(upcasters[0].to_version, 2);
    assert_eq!(upcasters[1].from_version, 2);
    assert_eq!(upcasters[1].to_version, 3);
}

#[test]
fn hydrate_v2_from_v1_events() {
    let mut v1 = TodoV1::default();
    v1.initialize("t1".into(), "alice".into(), "Buy milk".into());
    v1.complete();

    let mut entity = Entity::new();
    entity.load_from_history(v1.entity.events().to_vec());

    let v2: TodoV2 = hydrate(entity).unwrap();
    assert_eq!(v2.entity.id(), "t1");
    assert_eq!(v2.user_id, "alice");
    assert_eq!(v2.priority, 0);
    assert!(v2.completed);
}

#[test]
fn hydrate_v3_from_v1_events_chains_upcasters() {
    let mut v1 = TodoV1::default();
    v1.initialize("t1".into(), "bob".into(), "Walk dog".into());

    let mut entity = Entity::new();
    entity.load_from_history(v1.entity.events().to_vec());

    let v3: TodoV3 = hydrate(entity).unwrap();
    assert_eq!(v3.priority, 0);
    assert_eq!(v3.due_date, "");
}

#[test]
fn hydrate_v3_from_v2_events() {
    let mut v2 = TodoV2::default();
    v2.initialize("t1".into(), "carol".into(), "Read".into(), 5);

    let mut entity = Entity::new();
    entity.load_from_history(v2.entity.events().to_vec());

    let v3: TodoV3 = hydrate(entity).unwrap();
    assert_eq!(v3.priority, 5);
    assert_eq!(v3.due_date, "");
}

#[test]
fn hydrate_v3_native_no_upcasting() {
    let mut v3 = TodoV3::default();
    v3.initialize("t1".into(), "dave".into(), "Cook".into(), 2, "2025-12-31".into());

    let mut entity = Entity::new();
    entity.load_from_history(v3.entity.events().to_vec());

    let loaded: TodoV3 = hydrate(entity).unwrap();
    assert_eq!(loaded.priority, 2);
    assert_eq!(loaded.due_date, "2025-12-31");
}

#[test]
fn repo_roundtrip_v1_to_v2() {
    let repo = HashMapRepository::new();
    let mut v1 = TodoV1::default();
    v1.initialize("t1".into(), "frank".into(), "Shop".into());
    repo.commit(&mut v1.entity).unwrap();

    let v2_repo = repo.aggregate::<TodoV2>();
    let loaded = v2_repo.get("t1").unwrap().unwrap();
    assert_eq!(loaded.user_id, "frank");
    assert_eq!(loaded.priority, 0);
}

#[test]
fn typed_events_have_correct_names() {
    assert_eq!(
        TodoV1Event::Initialized {
            id: "".into(),
            user_id: "".into(),
            task: "".into(),
        }
        .event_name(),
        "Initialized"
    );
    assert_eq!(TodoV1Event::Completed.event_name(), "Completed");
}

#[test]
fn v2_try_from_event_record() {
    let payload = bitcode::serialize(&(
        "t1".to_string(),
        "alice".to_string(),
        "Task".to_string(),
        3u8,
    ))
    .unwrap();
    let record = sourced_rust::EventRecord::new_versioned("Initialized", payload, 1, 2);
    let event = TodoV2Event::try_from(&record).unwrap();
    match event {
        TodoV2Event::Initialized {
            id,
            user_id,
            task,
            priority,
        } => {
            assert_eq!(id, "t1");
            assert_eq!(user_id, "alice");
            assert_eq!(task, "Task");
            assert_eq!(priority, 3);
        }
        _ => panic!("Expected Initialized"),
    }
}

#[test]
fn mixed_v1_events_upcasted_to_v2() {
    let mut v1 = TodoV1::default();
    v1.initialize("t1".into(), "eve".into(), "Test".into());
    v1.complete();

    let mut entity = Entity::new();
    entity.load_from_history(v1.entity.events().to_vec());

    let v2: TodoV2 = hydrate(entity).unwrap();
    assert_eq!(v2.priority, 0);
    assert!(v2.completed);
}
