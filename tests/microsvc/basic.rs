//! Basic microsvc integration tests — exercises dispatch with a real repository.

use serde_json::json;
use sourced_rust::microsvc::{HandlerError, Service, Session};
use sourced_rust::{AggregateBuilder, HashMapRepository};

use crate::support::{Counter, CreateCounter, DecrementCounter, IncrementCounter};

#[test]
fn full_lifecycle() {
    let service = Service::new(HashMapRepository::new())
        .command("counter.create", |ctx| {
            let input = ctx.input::<CreateCounter>()?;
            let counter_repo = ctx.repo().clone().aggregate::<Counter>();
            let mut counter = Counter::default();
            counter.create(input.id.clone());
            counter_repo.commit(&mut counter)?;
            Ok(json!({ "id": input.id }))
        })
        .command("counter.increment", |ctx| {
            let input = ctx.input::<IncrementCounter>()?;
            let counter_repo = ctx.repo().clone().aggregate::<Counter>();
            let mut counter: Counter = counter_repo
                .get(&input.id)?
                .ok_or_else(|| HandlerError::NotFound(input.id.clone()))?;
            counter.increment(input.amount);
            counter_repo.commit(&mut counter)?;
            Ok(json!({ "value": counter.value }))
        })
        .command("counter.decrement", |ctx| {
            let input = ctx.input::<DecrementCounter>()?;
            let counter_repo = ctx.repo().clone().aggregate::<Counter>();
            let mut counter: Counter = counter_repo
                .get(&input.id)?
                .ok_or_else(|| HandlerError::NotFound(input.id.clone()))?;
            counter.decrement(input.amount);
            counter_repo.commit(&mut counter)?;
            Ok(json!({ "value": counter.value }))
        });

    // Create
    let result = service
        .dispatch("counter.create", json!({ "id": "c1" }), Session::new())
        .unwrap();
    assert_eq!(result, json!({ "id": "c1" }));

    // Increment twice
    let result = service
        .dispatch(
            "counter.increment",
            json!({ "id": "c1", "amount": 5 }),
            Session::new(),
        )
        .unwrap();
    assert_eq!(result, json!({ "value": 5 }));

    service
        .dispatch(
            "counter.increment",
            json!({ "id": "c1", "amount": 3 }),
            Session::new(),
        )
        .unwrap();

    // Decrement
    let result = service
        .dispatch(
            "counter.decrement",
            json!({ "id": "c1", "amount": 2 }),
            Session::new(),
        )
        .unwrap();
    assert_eq!(result, json!({ "value": 6 }));

    // Verify final state via repo
    let counter_repo = service.repo().clone().aggregate::<Counter>();
    let counter: Counter = counter_repo.get("c1").unwrap().unwrap();
    assert_eq!(counter.value, 6);
}
