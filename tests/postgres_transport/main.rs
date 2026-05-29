//! Postgres transport adapter integration tests.
//!
//! Exercises `OutboxSource<PostgresOutboxStore>` — the Postgres "starter"
//! durable transport — against a real Postgres: claim (`FOR UPDATE SKIP LOCKED`
//! with a lease), dispatch, and settle by row status. Skips when `DATABASE_URL`
//! is unset.
#![cfg(feature = "postgres")]

#[path = "../support/postgres.rs"]
mod postgres;

use std::sync::{Arc, Mutex};

use serde_json::json;
use sourced_rust::microsvc::transport::{
    run_source, AsyncMessageSource, OutboxSource, ReceivedMessage, RunOptions,
};
use sourced_rust::microsvc::Service;
use sourced_rust::{
    AsyncCommitBatch, AsyncOutboxStore, AsyncTransactionalCommit, OutboxMessage,
    OutboxMessageStatus, PostgresOutboxStore, PostgresRepository,
};

const SKIP: &str = "skipping postgres transport test";

async fn enqueue(repo: &PostgresRepository, id: &str, name: &str) {
    let mut batch = AsyncCommitBatch::empty();
    batch
        .outbox_messages
        .push(OutboxMessage::create(id, name, b"{}".to_vec()).unwrap());
    repo.commit_batch_async(batch)
        .await
        .expect("outbox row should commit");
}

async fn status(store: &PostgresOutboxStore, id: &str) -> Option<OutboxMessageStatus> {
    for s in [
        OutboxMessageStatus::Pending,
        OutboxMessageStatus::InFlight,
        OutboxMessageStatus::Published,
        OutboxMessageStatus::Failed,
    ] {
        if store
            .messages_by_status_async(s.clone())
            .await
            .unwrap()
            .iter()
            .any(|m| m.id() == id)
        {
            return Some(s);
        }
    }
    None
}

fn recording_service(handled: Arc<Mutex<Vec<String>>>) -> Arc<Service<()>> {
    Arc::new(Service::new(()).event("evt").handle(move |ctx| {
        handled
            .lock()
            .unwrap()
            .push(ctx.message().id().unwrap_or_default().to_string());
        Ok(json!({}))
    }))
}

#[tokio::test]
async fn outbox_source_run_drains_and_completes() {
    let Some(schema) = postgres::PostgresTestSchema::create_from_env("pg_tx_drain", SKIP).await
    else {
        return;
    };
    let repo = schema.repository().await;
    enqueue(&repo, "m1", "evt").await;
    enqueue(&repo, "m2", "evt").await;
    let store = Arc::new(repo.outbox_store());

    let handled = Arc::new(Mutex::new(Vec::new()));
    let service = recording_service(handled.clone());
    run_source(
        service,
        OutboxSource::new(store.clone(), "pg-drain", 3),
        RunOptions::idempotent(),
    )
    .await
    .unwrap();

    let mut ids = handled.lock().unwrap().clone();
    ids.sort();
    assert_eq!(ids, vec!["m1".to_string(), "m2".to_string()]);
    assert_eq!(
        status(&store, "m1").await,
        Some(OutboxMessageStatus::Published)
    );
    assert_eq!(
        status(&store, "m2").await,
        Some(OutboxMessageStatus::Published)
    );
}

#[tokio::test]
async fn concurrent_sources_process_each_row_once() {
    let Some(schema) =
        postgres::PostgresTestSchema::create_from_env("pg_tx_concurrent", SKIP).await
    else {
        return;
    };
    let repo = schema.repository().await;
    let ids: Vec<String> = (0..20).map(|i| format!("c{i}")).collect();
    for id in &ids {
        enqueue(&repo, id, "evt").await;
    }
    let store = Arc::new(repo.outbox_store());

    let handled = Arc::new(Mutex::new(Vec::new()));
    let run = |worker: &'static str| {
        run_source(
            recording_service(handled.clone()),
            OutboxSource::new(store.clone(), worker, 3),
            RunOptions::idempotent(),
        )
    };
    // Two competing consumers drain concurrently; SKIP LOCKED guarantees each
    // row is claimed (and handled) exactly once.
    let (a, b) = tokio::join!(run("worker-a"), run("worker-b"));
    a.unwrap();
    b.unwrap();

    let mut got = handled.lock().unwrap().clone();
    got.sort();
    let unique = {
        let mut u = got.clone();
        u.dedup();
        u
    };
    assert_eq!(got, unique, "no row handled more than once");
    assert_eq!(unique.len(), ids.len(), "every row handled");
}

#[tokio::test]
async fn nack_releases_then_a_later_claim_completes() {
    let Some(schema) = postgres::PostgresTestSchema::create_from_env("pg_tx_retry", SKIP).await
    else {
        return;
    };
    let repo = schema.repository().await;
    enqueue(&repo, "m1", "evt").await;
    let store = Arc::new(repo.outbox_store());

    // First claim, nack -> released to pending (attempts incremented).
    let mut source = OutboxSource::new(store.clone(), "pg-retry", 5);
    let received = source.recv().await.unwrap().expect("a claimable row");
    received.nack("transient").await.unwrap();
    assert_eq!(
        status(&store, "m1").await,
        Some(OutboxMessageStatus::Pending)
    );

    // A later claim completes it.
    let mut source2 = OutboxSource::new(store.clone(), "pg-retry-2", 5);
    let received2 = source2.recv().await.unwrap().expect("a re-claimable row");
    received2.ack().await.unwrap();
    assert_eq!(
        status(&store, "m1").await,
        Some(OutboxMessageStatus::Published)
    );
}

#[tokio::test]
async fn dead_letter_marks_row_failed() {
    let Some(schema) = postgres::PostgresTestSchema::create_from_env("pg_tx_dlq", SKIP).await
    else {
        return;
    };
    let repo = schema.repository().await;
    enqueue(&repo, "m1", "evt").await;
    let store = Arc::new(repo.outbox_store());

    let mut source = OutboxSource::new(store.clone(), "pg-dlq", 3);
    let received = source.recv().await.unwrap().expect("a claimable row");
    received.dead_letter("poison").await.unwrap();
    assert_eq!(
        status(&store, "m1").await,
        Some(OutboxMessageStatus::Failed)
    );
}
