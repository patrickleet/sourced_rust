//! Tests demonstrating the handler file convention.
//!
//! Each handler lives in its own file under `handlers/` and exports:
//! - `COMMAND: &str` — the command name
//! - `guard(ctx) -> bool` — input validation
//! - `handle(ctx) -> Result<Value, HandlerError>` — the handler
//!
//! Registration uses the `register_handlers!` macro.

use serde_json::json;
use sourced_rust::microsvc::{Service, Session};
use sourced_rust::{GetAggregate, HashMapRepository};

use crate::support::Counter;
use crate::handlers;

#[test]
fn register_handlers_from_modules() {
    let service = sourced_rust::register_handlers!(
        Service::new(HashMapRepository::new()),
        handlers::counter_create,
        handlers::counter_increment,
    );

    // Verify both commands are registered
    let mut cmds = service.commands();
    cmds.sort();
    assert_eq!(cmds, vec!["counter.create", "counter.increment"]);

    // Create a counter
    let result = service
        .dispatch("counter.create", json!({ "id": "c1" }), Session::new())
        .unwrap();
    assert_eq!(result, json!({ "id": "c1" }));

    // Increment it
    let result = service
        .dispatch(
            "counter.increment",
            json!({ "id": "c1", "amount": 10 }),
            Session::new(),
        )
        .unwrap();
    assert_eq!(result, json!({ "id": "c1", "value": 10 }));

    // Verify state
    let counter: Counter = service.repo().get_aggregate("c1").unwrap().unwrap();
    assert_eq!(counter.value, 10);
}

#[test]
fn handler_module_guard_rejects() {
    let service = sourced_rust::register_handlers!(
        Service::new(HashMapRepository::new()),
        handlers::counter_create,
    );

    // Missing "id" field — guard should reject
    let result = service.dispatch("counter.create", json!({ "wrong": 1 }), Session::new());
    assert!(result.is_err());
}

#[test]
fn handler_rejects_duplicate_create() {
    let service = sourced_rust::register_handlers!(
        Service::new(HashMapRepository::new()),
        handlers::counter_create,
    );

    // First create succeeds
    service
        .dispatch("counter.create", json!({ "id": "c1" }), Session::new())
        .unwrap();

    // Second create fails
    let result = service.dispatch("counter.create", json!({ "id": "c1" }), Session::new());
    assert!(result.is_err());
}
