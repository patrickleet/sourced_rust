//! Basic microsvc integration tests — exercises dispatch with a real repository.

use distributed::microsvc::{Context, HandlerError, Routes, Service, Session};
use distributed::{AggregateBuilder, InMemoryRepository};
use serde_json::json;

use crate::models::counter::{Counter, CreateCounter, DecrementCounter, IncrementCounter};

#[tokio::test]
async fn full_lifecycle() {
    let repo = InMemoryRepository::new();
    let counter_repo = repo.clone().aggregate::<Counter>();
    let service = Service::new().routes(
        Routes::new()
            .with_dependencies(repo)
            .command("counter.initialize")
            .handle(|ctx: &Context<InMemoryRepository>| {
                let input = ctx.input::<CreateCounter>();
                let counter_repo = ctx.repo().clone().aggregate::<Counter>();
                async move {
                    let input = input?;
                    let mut counter = Counter::default();
                    counter.create(input.id.clone())?;
                    counter_repo.commit(&mut counter).await?;
                    Ok(json!({ "id": input.id }))
                }
            })
            .command("counter.increment")
            .handle(|ctx: &Context<InMemoryRepository>| {
                let input = ctx.input::<IncrementCounter>();
                let counter_repo = ctx.repo().clone().aggregate::<Counter>();
                async move {
                    let input = input?;
                    let mut counter: Counter = counter_repo
                        .get(&input.id)
                        .await?
                        .ok_or_else(|| HandlerError::NotFound(input.id.clone()))?;
                    counter.increment(input.amount)?;
                    counter_repo.commit(&mut counter).await?;
                    Ok(json!({ "value": counter.value }))
                }
            })
            .command("counter.decrement")
            .handle(|ctx: &Context<InMemoryRepository>| {
                let input = ctx.input::<DecrementCounter>();
                let counter_repo = ctx.repo().clone().aggregate::<Counter>();
                async move {
                    let input = input?;
                    let mut counter: Counter = counter_repo
                        .get(&input.id)
                        .await?
                        .ok_or_else(|| HandlerError::NotFound(input.id.clone()))?;
                    counter.decrement(input.amount)?;
                    counter_repo.commit(&mut counter).await?;
                    Ok(json!({ "value": counter.value }))
                }
            }),
    );

    // Create
    let result = service
        .dispatch("counter.initialize", json!({ "id": "c1" }), Session::new())
        .await
        .unwrap();
    assert_eq!(result, json!({ "id": "c1" }));

    // Increment twice
    let result = service
        .dispatch(
            "counter.increment",
            json!({ "id": "c1", "amount": 5 }),
            Session::new(),
        )
        .await
        .unwrap();
    assert_eq!(result, json!({ "value": 5 }));

    service
        .dispatch(
            "counter.increment",
            json!({ "id": "c1", "amount": 3 }),
            Session::new(),
        )
        .await
        .unwrap();

    // Decrement
    let result = service
        .dispatch(
            "counter.decrement",
            json!({ "id": "c1", "amount": 2 }),
            Session::new(),
        )
        .await
        .unwrap();
    assert_eq!(result, json!({ "value": 6 }));

    // Verify final state via repo
    let counter: Counter = counter_repo.get("c1").await.unwrap().unwrap();
    assert_eq!(counter.value, 6);
}
