use serde::{Deserialize, Serialize};
use sourced_rust::bus::{Event, Publisher, Subscriber};
use sourced_rust::{
    InMemoryQueue, InMemoryReadModelStore, ReadModel, ReadModelError, ReadModelStore,
    ReadModelWritePlanBuilder, ReadModelWritePlanStore, RowKey, RowValue,
};

const CONSUMER: &str = "counter-projection";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ReadModel)]
#[collection("counter_views")]
struct CounterView {
    #[id]
    id: String,
    value: i32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ReadModel)]
#[table("relational_counters")]
struct RelationalCounter {
    #[id]
    id: String,
    value: i32,
}

fn counter_session(view: &CounterView, message_id: &str) -> ReadModelWritePlanBuilder {
    let mut session = ReadModelWritePlanBuilder::new();
    session
        .document(view)
        .unwrap()
        .mark_processed(CONSUMER, message_id);
    session
}

fn relational_counter_key(id: &str) -> RowKey {
    RowKey::new([("id", RowValue::String(id.into()))])
}

#[test]
fn standalone_session_commit_applies_document_and_marks_processed() {
    let store = InMemoryReadModelStore::new();
    assert!(store.read_model_capabilities().processed_messages);
    let view = CounterView {
        id: "counter-1".into(),
        value: 1,
    };

    let outcome = counter_session(&view, "message-1").commit(&store).unwrap();

    assert!(outcome.was_applied());
    assert!(store.is_processed(CONSUMER, "message-1").unwrap());
    let loaded = store
        .get_by_primary_key::<CounterView>("counter-1")
        .unwrap()
        .unwrap();
    assert_eq!(loaded.data, view);
    assert_eq!(loaded.version, 1);
}

#[test]
fn duplicate_processed_message_skips_mutations_idempotently() {
    let store = InMemoryReadModelStore::new();
    let original = CounterView {
        id: "counter-1".into(),
        value: 1,
    };
    counter_session(&original, "message-1")
        .commit(&store)
        .unwrap();

    let duplicate_update = CounterView {
        id: "counter-1".into(),
        value: 99,
    };
    let outcome = counter_session(&duplicate_update, "message-1")
        .commit(&store)
        .unwrap();

    assert!(outcome.was_skipped());
    assert_eq!(
        outcome
            .duplicate_message()
            .map(|mark| mark.message_id.as_str()),
        Some("message-1")
    );
    let loaded = store
        .get_by_primary_key::<CounterView>("counter-1")
        .unwrap()
        .unwrap();
    assert_eq!(loaded.data, original);
    assert_eq!(loaded.version, 1);
}

#[test]
fn read_model_write_and_processed_mark_are_atomic() {
    let store = InMemoryReadModelStore::new();
    let view = CounterView {
        id: "counter-1".into(),
        value: 1,
    };
    let row = RelationalCounter {
        id: "counter-1".into(),
        value: 1,
    };
    let mut session = ReadModelWritePlanBuilder::new();
    session
        .document(&view)
        .unwrap()
        .mark_processed(CONSUMER, "message-1")
        .expect_version::<RelationalCounter>(relational_counter_key("counter-1"), 99)
        .unwrap()
        .upsert(&row)
        .unwrap();

    let err = session.commit(&store).unwrap_err();

    assert!(matches!(err, ReadModelError::NotFound { .. }));
    assert!(!store.is_processed(CONSUMER, "message-1").unwrap());
    assert!(store
        .get_by_primary_key::<CounterView>("counter-1")
        .unwrap()
        .is_none());
}

#[test]
fn ack_happens_only_after_successful_standalone_commit() {
    let queue = InMemoryQueue::new();
    let store = InMemoryReadModelStore::new();

    queue
        .publish(Event::with_string_payload(
            "message-fail",
            "CounterChanged",
            "{}",
        ))
        .unwrap();
    let failed = queue.poll(0).unwrap().unwrap();
    let row = RelationalCounter {
        id: "counter-1".into(),
        value: 1,
    };
    let mut failed_session = ReadModelWritePlanBuilder::new();
    failed_session
        .expect_version::<RelationalCounter>(relational_counter_key("counter-1"), 99)
        .unwrap()
        .upsert(&row)
        .unwrap()
        .mark_processed(CONSUMER, &failed.id);

    let err = failed_session.commit(&store).unwrap_err();

    assert!(matches!(err, ReadModelError::NotFound { .. }));
    assert!(queue.acknowledged().is_empty());
    assert!(!store.is_processed(CONSUMER, &failed.id).unwrap());

    queue
        .publish(Event::with_string_payload(
            "message-ok",
            "CounterChanged",
            "{}",
        ))
        .unwrap();
    let succeeded = queue.poll(0).unwrap().unwrap();
    let view = CounterView {
        id: "counter-1".into(),
        value: 2,
    };
    let outcome = counter_session(&view, &succeeded.id)
        .commit(&store)
        .unwrap();
    assert!(outcome.was_applied());

    queue.ack(&succeeded.id).unwrap();

    assert_eq!(queue.acknowledged(), vec!["message-ok"]);
    assert!(store.is_processed(CONSUMER, &succeeded.id).unwrap());
}
