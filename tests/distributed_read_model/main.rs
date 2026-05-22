//! Distributed read-model service example.
//!
//! This demonstrates a distributed CQRS deployment shape:
//! - the account model service owns the event-sourced aggregate and outbox
//! - the account summary projector owns read-model updates
//! - a separate query process reads from the projected read-model store
//! - the write side and projector are connected only through the bus
//!
//! The test uses threads and `InMemoryQueue` as process stand-ins. In a real
//! deployment, each service would use its own process, a shared broker, and a
//! shared read-model database. A query API such as Hasura can sit in front of
//! that database while the read-model worker keeps the tables updated.
//!
//! The model service is a `microsvc::Service`, so the same command handlers can
//! be exposed through direct dispatch, HTTP, or gRPC. The query side is not a
//! command handler; it just reads the projected store.

mod handlers;
mod models;
mod query_process;
mod read_model;

use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use models::aggregates::account::{Account, DepositMoney, OpenAccount};
use models::readmodels::account_summary::AccountSummary;
use query_process::AccountSummaryQueryProcess;
use read_model::{start_account_summary_service, wait_for_summary};
use sourced_rust::microsvc::{Service, Session};
use sourced_rust::{
    AggregateBuilder, AggregateRepository, GetAggregate, HashMapRepository, InMemoryQueue,
    OutboxWorkerThread, Queueable, QueuedRepository, ReadModelsExt,
};

pub(crate) type AccountRepo = AggregateRepository<QueuedRepository<HashMapRepository>, Account>;

fn account_model_service(repo: AccountRepo) -> Arc<Service<AccountRepo>> {
    Arc::new(sourced_rust::register_handlers!(
        Service::new(repo),
        handlers::account_open,
        handlers::account_deposit,
    ))
}

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

#[cfg(feature = "http")]
async fn start_http_service<R: Send + Sync + 'static>(service: Arc<Service<R>>) -> String {
    let app = sourced_rust::microsvc::router(service);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("HTTP test server should bind");
    let addr = listener
        .local_addr()
        .expect("HTTP test server should expose local address");

    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("HTTP test server should serve");
    });

    format!("http://{addr}")
}

#[cfg(feature = "grpc")]
async fn start_grpc_service<R: Send + Sync + 'static>(
    service: Arc<Service<R>>,
) -> sourced_rust::microsvc::grpc::CommandServiceClient<tonic::transport::Channel> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("gRPC test server should bind");
    let addr = listener
        .local_addr()
        .expect("gRPC test server should expose local address");
    let grpc_svc = sourced_rust::microsvc::grpc_server(service);

    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(grpc_svc)
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
            .await
            .expect("gRPC test server should serve");
    });

    sourced_rust::microsvc::grpc::CommandServiceClient::connect(format!("http://{addr}"))
        .await
        .expect("gRPC test client should connect")
}

#[test]
fn write_model_service_feeds_separate_read_model_service() {
    let queue = InMemoryQueue::new();

    let write_store = HashMapRepository::new();
    let account_repo = write_store.clone().queued().aggregate::<Account>();
    let model_service = account_model_service(account_repo);

    let outbox_worker =
        OutboxWorkerThread::spawn(write_store.clone(), queue.clone(), Duration::from_millis(5));

    let read_store = HashMapRepository::new();
    let read_model_service = start_account_summary_service(queue.clone(), read_store.clone());
    let query_process = AccountSummaryQueryProcess::new(read_store.clone());

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

    let queried_summary = query_process
        .get("acct-1")
        .expect("query process should read projected account summary")
        .expect("query process should find projected account summary");
    assert_eq!(queried_summary.owner.as_deref(), Some("Ada Lovelace"));
    assert_eq!(queried_summary.balance_cents, 2500);
    assert_eq!(queried_summary.deposit_count, 1);
    assert!(query_process
        .get("missing-account")
        .expect("query process should read projected account summary")
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

    let read_side_account = read_store
        .get_aggregate::<Account>("acct-1")
        .expect("read-side aggregate lookup should succeed");
    assert!(
        read_side_account.is_none(),
        "read-model service should not own the account aggregate"
    );

    read_model_service.stop();
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
    let model_service = account_model_service(account_repo);
    let model_base = start_http_service(model_service.clone()).await;

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
    let model_service = account_model_service(account_repo);
    let mut model_client = start_grpc_service(model_service.clone()).await;

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
