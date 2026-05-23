//! Distributed read-model service example.
//!
//! This demonstrates a distributed CQRS deployment shape:
//! - the account model service owns the event-sourced aggregate and outbox
//! - the account summary projector owns read-model updates
//! - a separate query service reads from the projected read-model store
//! - the write side and projector are connected only through the bus
//!
//! The test uses threads and `InMemoryQueue` as service stand-ins. In a real
//! deployment, each service would use its own process, a shared broker, and a
//! shared read-model database. A query API such as Hasura can sit in front of
//! that database while the read-model worker keeps the tables updated.
//!
//! The model service is a `microsvc::Service`, so the same command handlers can
//! be exposed through direct dispatch, HTTP, or gRPC. The query side is not a
//! command handler; it just reads the projected store.

mod account_service;
mod projections_service;
mod query_service;

use std::thread;
use std::time::{Duration, Instant};

use account_service::{Account, DepositMoney, OpenAccount};
use projections_service::{
    start_account_summary_projection_service, wait_for_summary, ACCOUNT_SUMMARY_CONSUMER,
};
use query_service::{AccountSummary, AccountSummaryQueryService};
use sourced_rust::microsvc::Session;
use sourced_rust::{
    AggregateBuilder, HashMapRepository, InMemoryQueue, InMemoryReadModelStore, OutboxWorkerThread,
    Queueable, ReadModelSessionStore, ReadModelsExt,
};

fn wait_for_published_events(queue: &InMemoryQueue, expected_count: usize) {
    let deadline = Instant::now() + Duration::from_secs(10);

    loop {
        if queue.len() >= expected_count {
            return;
        }

        assert!(
            Instant::now() < deadline,
            "timed out waiting for outbox worker to publish account events"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn write_model_service_feeds_separate_read_model_service() {
    let queue = InMemoryQueue::new();

    let write_store = HashMapRepository::new();
    let account_repo = write_store.clone().queued().aggregate::<Account>();
    let model_service = account_service::model_service(account_repo);

    let outbox_worker =
        OutboxWorkerThread::spawn(write_store.clone(), queue.clone(), Duration::from_millis(5));

    let read_store = InMemoryReadModelStore::new();
    let projections_service =
        start_account_summary_projection_service(queue.clone(), read_store.clone());
    let query_service = AccountSummaryQueryService::new(read_store.clone());

    model_service
        .dispatch(
            "account.open",
            serde_json::to_value(OpenAccount {
                id: "acct-1".to_string(),
                owner: "Ada Lovelace".to_string(),
            })
            .expect("open command should encode"),
            Session::new(),
        )
        .expect("open command should dispatch");
    model_service
        .dispatch(
            "account.deposit",
            serde_json::to_value(DepositMoney {
                id: "acct-1".to_string(),
                amount_cents: 2500,
            })
            .expect("deposit command should encode"),
            Session::new(),
        )
        .expect("deposit command should dispatch");

    wait_for_published_events(&queue, 2);

    let summary = wait_for_summary(&read_store, "acct-1", |summary| {
        summary.owner.as_deref() == Some("Ada Lovelace")
            && summary.balance_cents == 2500
            && summary.deposit_count == 1
    });

    assert_eq!(summary.owner.as_deref(), Some("Ada Lovelace"));
    assert_eq!(summary.balance_cents, 2500);
    assert_eq!(summary.deposit_count, 1);
    for event in queue.events() {
        assert!(
            read_store
                .is_processed(ACCOUNT_SUMMARY_CONSUMER, &event.id)
                .expect("processed-message lookup should succeed"),
            "read-model service should mark projected events processed before they are acknowledged"
        );
    }

    let queried_summary = query_service
        .get("acct-1")
        .expect("query service should read projected account summary")
        .expect("query service should find projected account summary");
    assert_eq!(queried_summary.owner.as_deref(), Some("Ada Lovelace"));
    assert_eq!(queried_summary.balance_cents, 2500);
    assert_eq!(queried_summary.deposit_count, 1);
    assert!(query_service
        .get("missing-account")
        .expect("query service should read projected account summary")
        .is_none());

    let mut model_commands = model_service.commands();
    model_commands.sort();
    assert_eq!(
        model_commands,
        vec!["account.deposit", "account.open"],
        "model service should expose only write-side commands"
    );

    let write_side_account = model_service
        .repo()
        .peek("acct-1")
        .expect("write-side aggregate load should succeed")
        .expect("write-side aggregate should exist");
    assert_eq!(write_side_account.balance_cents, 2500);

    let write_side_summary = write_store
        .read_models::<AccountSummary>()
        .get("acct-1")
        .expect("write-side read model lookup should succeed");
    assert!(
        write_side_summary.is_none(),
        "write-side service should not own the account summary projection"
    );

    projections_service.stop();
    let worker_stats = outbox_worker
        .stop()
        .expect("outbox worker should stop cleanly");

    assert!(worker_stats.messages_published >= 2);
}

#[cfg(feature = "http")]
#[tokio::test]
async fn model_commands_can_be_http_service() {
    let write_store = HashMapRepository::new();
    let account_repo = write_store.clone().queued().aggregate::<Account>();
    let model_service = account_service::model_service(account_repo);
    let model_base = account_service::start_http_service(model_service.clone()).await;

    let client = reqwest::Client::new();
    let open = client
        .post(format!("{model_base}/account.open"))
        .json(&OpenAccount {
            id: "acct-http".to_string(),
            owner: "Grace Hopper".to_string(),
        })
        .send()
        .await
        .expect("HTTP model service should accept open request");
    assert_eq!(open.status(), 200);

    let deposit = client
        .post(format!("{model_base}/account.deposit"))
        .json(&DepositMoney {
            id: "acct-http".to_string(),
            amount_cents: 4200,
        })
        .send()
        .await
        .expect("HTTP model service should accept deposit request");
    assert_eq!(deposit.status(), 200);

    let account = model_service
        .repo()
        .peek("acct-http")
        .expect("HTTP write-side aggregate load should succeed")
        .expect("HTTP write-side aggregate should exist");
    assert_eq!(account.balance_cents, 4200);
}

#[cfg(feature = "grpc")]
#[tokio::test]
async fn model_commands_can_be_grpc_service() {
    let write_store = HashMapRepository::new();
    let account_repo = write_store.clone().queued().aggregate::<Account>();
    let model_service = account_service::model_service(account_repo);
    let mut model_client = account_service::start_grpc_service(model_service.clone()).await;

    let open = model_client
        .dispatch(sourced_rust::microsvc::grpc::GrpcRequest {
            command: "account.open".to_string(),
            input: serde_json::to_string(&OpenAccount {
                id: "acct-grpc".to_string(),
                owner: "Katherine Johnson".to_string(),
            })
            .expect("open command should encode"),
            session_variables: Default::default(),
        })
        .await
        .expect("gRPC model service should accept open request")
        .into_inner();
    assert_eq!(open.status, 200);

    let deposit = model_client
        .dispatch(sourced_rust::microsvc::grpc::GrpcRequest {
            command: "account.deposit".to_string(),
            input: serde_json::to_string(&DepositMoney {
                id: "acct-grpc".to_string(),
                amount_cents: 7300,
            })
            .expect("deposit command should encode"),
            session_variables: Default::default(),
        })
        .await
        .expect("gRPC model service should accept deposit request")
        .into_inner();
    assert_eq!(deposit.status, 200);

    let account = model_service
        .repo()
        .peek("acct-grpc")
        .expect("gRPC write-side aggregate load should succeed")
        .expect("gRPC write-side aggregate should exist");
    assert_eq!(account.balance_cents, 7300);
}
