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

use distributed::bus::{
    run_source, AsyncMessageSource, Bus, BusConsumer, PostgresBus, ReceivedMessage, RunOptions,
};
use distributed::microsvc::{Context, Message, MessageKind, Service};
use distributed::OutboxSource;
use distributed::{
    AsyncOutboxStore, CommitBatch, OutboxMessage, OutboxMessageStatus, PostgresOutboxStore,
    PostgresRepository, TransactionalCommit,
};
use serde_json::json;

const SKIP: &str = "skipping postgres transport test";

async fn enqueue(repo: &PostgresRepository, id: &str, name: &str) {
    let mut batch = CommitBatch::empty();
    batch
        .outbox_messages
        .push(OutboxMessage::create(id, name, b"{}".to_vec()).unwrap());
    repo.commit_batch(batch)
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
    Arc::new(
        Service::new()
            .event("order.initialized")
            .handle(move |ctx: &Context<()>| {
                handled
                    .lock()
                    .unwrap()
                    .push(ctx.message().id().unwrap_or_default().to_string());
                async move { Ok(json!({})) }
            }),
    )
}

#[tokio::test]
async fn outbox_source_run_drains_and_completes() {
    let Some(schema) = postgres::PostgresTestSchema::create_from_env("pg_tx_drain", SKIP).await
    else {
        return;
    };
    let repo = schema.repository().await;
    enqueue(&repo, "m1", "order.initialized").await;
    enqueue(&repo, "m2", "order.initialized").await;
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
        enqueue(&repo, id, "order.initialized").await;
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
    enqueue(&repo, "m1", "order.initialized").await;
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
    enqueue(&repo, "m1", "order.initialized").await;
    let store = Arc::new(repo.outbox_store());

    let mut source = OutboxSource::new(store.clone(), "pg-dlq", 3);
    let received = source.recv().await.unwrap().expect("a claimable row");
    received.dead_letter("poison").await.unwrap();
    assert_eq!(
        status(&store, "m1").await,
        Some(OutboxMessageStatus::Failed)
    );
}

// ---- PostgresBus: send/listen (work queue) + publish/subscribe (log+offsets) ----

/// Service whose single handler records the message id; `kind` picks command vs
/// event registration.
fn recording_for(name: &str, kind: MessageKind, rec: Arc<Mutex<Vec<String>>>) -> Arc<Service<()>> {
    let leaked: &'static str = Box::leak(name.to_string().into_boxed_str());
    let builder = Service::new();
    let registered = match kind {
        MessageKind::Command => builder.command(leaked),
        MessageKind::Event => builder.event(leaked),
    };
    Arc::new(registered.handle(move |ctx: &Context<()>| {
        rec.lock()
            .unwrap()
            .push(ctx.message().id().unwrap_or_default().to_string());
        async move { Ok(json!({})) }
    }))
}

fn named_recording_for(
    service_name: &str,
    name: &str,
    kind: MessageKind,
    rec: Arc<Mutex<Vec<String>>>,
) -> Arc<Service<()>> {
    let leaked: &'static str = Box::leak(name.to_string().into_boxed_str());
    let builder = Service::new().named(service_name.to_string());
    let registered = match kind {
        MessageKind::Command => builder.command(leaked),
        MessageKind::Event => builder.event(leaked),
    };
    Arc::new(registered.handle(move |ctx: &Context<()>| {
        rec.lock()
            .unwrap()
            .push(ctx.message().id().unwrap_or_default().to_string());
        async move { Ok(json!({})) }
    }))
}

/// `send` + `listen`: the work queue is claimed `FOR UPDATE SKIP LOCKED`, so two
/// replicas sharing a `group` compete — each command handled exactly once.
#[tokio::test]
async fn bus_send_listen_is_point_to_point_across_a_group() {
    let Some(schema) = postgres::PostgresTestSchema::create_from_env("bus_pp", SKIP).await else {
        return;
    };
    let repo = schema.repository().await;
    let bus = PostgresBus::new(repo.pool().clone()).group("orders");
    bus.ensure_tables().await.expect("ensure tables");

    let total = 6;
    for i in 0..total {
        bus.send_message(
            Message::new("order.initialize", MessageKind::Command, b"{}".to_vec())
                .with_id(format!("c{i}")),
        )
        .await
        .expect("send command");
    }

    let rec = Arc::new(Mutex::new(Vec::new()));
    let bus_a = bus.clone();
    let bus_b = bus.clone();
    let (ra, rb) = tokio::join!(
        bus_a.listen(
            recording_for("order.initialize", MessageKind::Command, rec.clone()),
            RunOptions::idempotent()
        ),
        bus_b.listen(
            recording_for("order.initialize", MessageKind::Command, rec.clone()),
            RunOptions::idempotent()
        ),
    );
    ra.expect("replica a drains");
    rb.expect("replica b drains");

    let mut ids = rec.lock().unwrap().clone();
    ids.sort();
    let expected: Vec<String> = (0..total).map(|i| format!("c{i}")).collect();
    assert_eq!(
        ids, expected,
        "every command handled exactly once across the group"
    );
}

/// `publish` + `subscribe`: each `group` has its own log offset, so every group
/// reads the full log — fan-out. nacked entries do not advance the offset.
#[tokio::test]
async fn bus_publish_subscribe_fans_out_across_groups() {
    let Some(schema) = postgres::PostgresTestSchema::create_from_env("bus_fan", SKIP).await else {
        return;
    };
    let repo = schema.repository().await;
    let pool = repo.pool().clone();
    let producer = PostgresBus::new(pool.clone()).group("producer");
    producer.ensure_tables().await.expect("ensure tables");

    let total = 4;
    for i in 0..total {
        producer
            .publish_message(
                Message::new("order.initialized", MessageKind::Event, b"{}".to_vec())
                    .with_id(format!("e{i}")),
            )
            .await
            .expect("publish event");
    }

    let expected: Vec<String> = (0..total).map(|i| format!("e{i}")).collect();
    for group in ["projections", "audit"] {
        let bus = PostgresBus::new(pool.clone()).group(group);
        let rec = Arc::new(Mutex::new(Vec::new()));
        bus.subscribe(
            recording_for("order.initialized", MessageKind::Event, rec.clone()),
            RunOptions::idempotent(),
        )
        .await
        .expect("subscriber drains");
        let mut ids = rec.lock().unwrap().clone();
        ids.sort();
        assert_eq!(ids, expected, "group {group} sees every event");
    }
}

#[tokio::test]
async fn bus_subscribe_uses_named_service_as_consumer_group() {
    let Some(schema) = postgres::PostgresTestSchema::create_from_env("bus_named_group", SKIP).await
    else {
        return;
    };
    let repo = schema.repository().await;
    let pool = repo.pool().clone();
    let producer = PostgresBus::new(pool.clone());
    producer.ensure_tables().await.expect("ensure tables");

    for i in 0..3 {
        producer
            .publish_message(
                Message::new("order.initialized", MessageKind::Event, b"{}".to_vec())
                    .with_id(format!("e{i}")),
            )
            .await
            .expect("publish event");
    }

    let rec = Arc::new(Mutex::new(Vec::new()));
    PostgresBus::new(pool)
        .subscribe(
            named_recording_for(
                "order-projection",
                "order.initialized",
                MessageKind::Event,
                rec.clone(),
            ),
            RunOptions::idempotent(),
        )
        .await
        .expect("subscriber drains");

    let mut ids = rec.lock().unwrap().clone();
    ids.sort();
    assert_eq!(
        ids,
        vec!["e0".to_string(), "e1".to_string(), "e2".to_string()]
    );
}
