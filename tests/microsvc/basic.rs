//! Basic microsvc integration tests — exercises dispatch with a real repository.

use distributed::microsvc::{Context, HandlerError, Routes, Service, Session};
use distributed::{AggregateBuilder, InMemoryRepository};
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

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

#[tokio::test]
async fn domain_event_fans_out_to_multiple_subscribers() {
    let projections = Arc::new(AtomicUsize::new(0));
    let policies = Arc::new(AtomicUsize::new(0));
    let projection_handler_count = projections.clone();
    let policy_handler_count = policies.clone();
    let service = Service::new()
        .routes(
            Routes::new()
                .with_dependencies(projections.clone())
                .event("metachangeset.opened")
                .handle(move |_ctx: &Context<Arc<AtomicUsize>>| {
                    let count = projection_handler_count.clone();
                    async move {
                        count.fetch_add(1, Ordering::SeqCst);
                        Ok(json!({ "subscriber": "projection" }))
                    }
                }),
        )
        .routes(
            Routes::new()
                .with_dependencies(policies.clone())
                .event("metachangeset.opened")
                .handle(move |_ctx: &Context<Arc<AtomicUsize>>| {
                    let count = policy_handler_count.clone();
                    async move {
                        count.fetch_add(1, Ordering::SeqCst);
                        Ok(json!({ "subscriber": "policy" }))
                    }
                }),
        );

    let message = distributed::microsvc::Message::new(
        "metachangeset.opened",
        distributed::microsvc::MessageKind::Event,
        b"{}".to_vec(),
    );
    service.dispatch_message(&message).await.unwrap();
    assert_eq!(projections.load(Ordering::SeqCst), 1);
    assert_eq!(policies.load(Ordering::SeqCst), 1);
}
