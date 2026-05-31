//! Tests demonstrating the handler file convention.
//!
//! Each handler lives in its own file under `handlers/` and exports:
//! - `COMMAND: &str` — the command name
//! - `guard(ctx) -> bool` — input validation
//! - `handle(ctx) -> Result<Value, HandlerError>` — the handler
//!
//! Registration uses the `register_handlers!` macro.

use distributed::microsvc::{Service, Session};
use distributed::{AsyncAggregateBuilder, HashMapRepository, OutboxStore, Queueable};
use serde_json::json;

use crate::handlers;
use crate::models::counter::Counter;

// ============================================================================
// Handler convention — register, dispatch, verify
// ============================================================================

#[tokio::test]
async fn register_handlers_and_dispatch() {
    let service = distributed::register_handlers!(
        Service::with_repo(HashMapRepository::new().queued_async().async_aggregate::<Counter>()),
        command handlers::counter_create,
        command handlers::counter_increment,
    );

    let mut cmds = service.command_names();
    cmds.sort();
    assert_eq!(cmds, vec!["counter.create", "counter.increment"]);

    // Create
    let result = service
        .dispatch("counter.create", json!({ "id": "c1" }), Session::new())
        .await
        .unwrap();
    assert_eq!(result, json!({ "id": "c1" }));

    // Increment
    let result = service
        .dispatch(
            "counter.increment",
            json!({ "id": "c1", "amount": 10 }),
            Session::new(),
        )
        .await
        .unwrap();
    assert_eq!(result, json!({ "id": "c1", "value": 10 }));

    // Verify state via repo
    let counter: Counter = service.repo().get("c1").await.unwrap().unwrap();
    assert_eq!(counter.value, 10);
}

#[tokio::test]
async fn guard_rejects_bad_input() {
    let service = distributed::register_handlers!(
        Service::with_repo(HashMapRepository::new().queued_async().async_aggregate::<Counter>()),
        command handlers::counter_create,
    );

    let result = service
        .dispatch("counter.create", json!({ "wrong": 1 }), Session::new())
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn handler_rejects_duplicate_create() {
    let service = distributed::register_handlers!(
        Service::with_repo(HashMapRepository::new().queued_async().async_aggregate::<Counter>()),
        command handlers::counter_create,
    );

    service
        .dispatch("counter.create", json!({ "id": "c1" }), Session::new())
        .await
        .unwrap();

    let result = service
        .dispatch("counter.create", json!({ "id": "c1" }), Session::new())
        .await;
    assert!(result.is_err());
}

// ============================================================================
// Outbox — handlers commit aggregate + outbox message atomically
// ============================================================================

#[tokio::test]
async fn create_persists_outbox_message() {
    let service = distributed::register_handlers!(
        Service::with_repo(HashMapRepository::new().queued_async().async_aggregate::<Counter>()),
        command handlers::counter_create,
    );

    let result = service
        .dispatch("counter.create", json!({ "id": "c1" }), Session::new())
        .await
        .unwrap();
    assert_eq!(result, json!({ "id": "c1" }));

    let inner = service.repo().repo().inner();

    // Aggregate was persisted
    let counter: Counter = service.repo().get("c1").await.unwrap().unwrap();
    assert_eq!(counter.value, 0);

    // Outbox message was persisted
    let pending = inner.outbox_store().pending().unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].event_type, "CounterCreated");
}

#[tokio::test]
async fn duplicate_create_leaves_single_outbox_message() {
    let service = distributed::register_handlers!(
        Service::with_repo(HashMapRepository::new().queued_async().async_aggregate::<Counter>()),
        command handlers::counter_create,
    );

    service
        .dispatch("counter.create", json!({ "id": "c1" }), Session::new())
        .await
        .unwrap();

    // Second create fails — no duplicate outbox message
    let result = service
        .dispatch("counter.create", json!({ "id": "c1" }), Session::new())
        .await;
    assert!(result.is_err());

    let pending = service
        .repo()
        .repo()
        .inner()
        .outbox_store()
        .pending()
        .unwrap();
    assert_eq!(pending.len(), 1);
}

#[tokio::test]
async fn increment_persists_outbox_message() {
    let service = distributed::register_handlers!(
        Service::with_repo(HashMapRepository::new().queued_async().async_aggregate::<Counter>()),
        command handlers::counter_create,
        command handlers::counter_increment,
    );

    service
        .dispatch("counter.create", json!({ "id": "c1" }), Session::new())
        .await
        .unwrap();

    service
        .dispatch(
            "counter.increment",
            json!({ "id": "c1", "amount": 7 }),
            Session::new(),
        )
        .await
        .unwrap();

    // Aggregate state is correct
    let counter: Counter = service.repo().get("c1").await.unwrap().unwrap();
    assert_eq!(counter.value, 7);

    // Both outbox messages were persisted
    let inner = service.repo().repo().inner();
    let pending = inner.outbox_store().pending().unwrap();
    assert_eq!(pending.len(), 2);
    let mut event_types: Vec<&str> = pending.iter().map(|m| m.event_type.as_str()).collect();
    event_types.sort();
    assert_eq!(event_types, vec!["CounterCreated", "CounterIncremented"]);
}
