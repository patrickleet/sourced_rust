//! Distributed read-model service example.
//!
//! This demonstrates a distributed CQRS deployment shape:
//! - the account model service owns the event-sourced aggregate and outbox
//! - the account summary projector owns read-model updates
//! - the account summary query service reads from the read-model store
//! - the write side and projector are connected only through the bus
//!
//! The test uses threads and `InMemoryQueue` as process stand-ins. In a real
//! deployment, each service would use its own process, a shared broker, and a
//! shared read-model database for the projector/query side.
//!
//! The model service and query service are both `microsvc::Service` instances,
//! so the same handlers can be exposed through direct dispatch, HTTP, or gRPC.

use std::sync::mpsc::{self, TryRecvError};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::json;
use sourced_rust::bus::{Bus, Event};
use sourced_rust::microsvc::{self, HandlerError, Service, Session};
use sourced_rust::{
    digest, AggregateBuilder, AggregateRepository, Entity, GetAggregate, HashMapRepository,
    InMemoryQueue, OutboxCommitExt, OutboxMessage, OutboxWorkerThread, Queueable, QueuedRepository,
    ReadModel, ReadModelsExt,
};

type AccountRepo = AggregateRepository<QueuedRepository<HashMapRepository>, Account>;

#[derive(Default)]
struct Account {
    entity: Entity,
    owner: String,
    balance_cents: i64,
    is_open: bool,
}

impl Account {
    #[digest("AccountOpened")]
    fn open(&mut self, account_id: String, owner: String) {
        self.entity.set_id(&account_id);
        self.owner = owner;
        self.balance_cents = 0;
        self.is_open = true;
    }

    #[digest("MoneyDeposited", when = self.is_open && amount_cents > 0)]
    fn deposit(&mut self, amount_cents: i64) {
        self.balance_cents += amount_cents;
    }
}

sourced_rust::aggregate!(Account, entity {
    "AccountOpened"(account_id, owner) => open,
    "MoneyDeposited"(amount_cents) => deposit,
});

#[derive(Clone, Debug, Deserialize, Serialize)]
struct OpenAccount {
    id: String,
    owner: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct DepositMoney {
    id: String,
    amount_cents: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct GetAccountSummary {
    account_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct AccountOpened {
    account_id: String,
    owner: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct MoneyDeposited {
    account_id: String,
    amount_cents: i64,
    new_balance_cents: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize, ReadModel)]
#[readmodel(collection = "account_summaries")]
struct AccountSummary {
    #[readmodel(id)]
    account_id: String,
    owner: Option<String>,
    balance_cents: i64,
    deposit_count: u32,
    projected_event_ids: Vec<String>,
}

impl AccountSummary {
    fn empty(account_id: &str) -> Self {
        Self {
            account_id: account_id.to_string(),
            owner: None,
            balance_cents: 0,
            deposit_count: 0,
            projected_event_ids: Vec::new(),
        }
    }

    fn has_projected(&self, event_id: &str) -> bool {
        self.projected_event_ids.iter().any(|id| id == event_id)
    }

    fn mark_projected(&mut self, event_id: &str) {
        self.projected_event_ids.push(event_id.to_string());
    }
}

fn account_model_service(repo: AccountRepo) -> Arc<Service<AccountRepo>> {
    let service = Service::new(repo)
        .command_guarded(
            "account.open",
            |ctx| ctx.has_fields(&["id", "owner"]),
            |ctx| {
                let input = ctx.input::<OpenAccount>()?;

                if ctx.repo().peek(&input.id)?.is_some() {
                    return Err(HandlerError::Rejected(format!(
                        "account {} already exists",
                        input.id
                    )));
                }

                let mut account = Account::default();
                account.open(input.id.clone(), input.owner.clone())?;

                let event = AccountOpened {
                    account_id: input.id.clone(),
                    owner: input.owner.clone(),
                };
                let mut outbox = OutboxMessage::encode_for_entity(
                    format!("{}:opened", input.id),
                    "AccountOpened",
                    &event,
                    &account.entity,
                )?;

                ctx.repo().outbox(&mut outbox).commit(&mut account)?;

                Ok(json!({ "id": input.id, "owner": input.owner }))
            },
        )
        .command_guarded(
            "account.deposit",
            |ctx| ctx.has_fields(&["id", "amount_cents"]),
            |ctx| {
                let input = ctx.input::<DepositMoney>()?;
                if input.amount_cents <= 0 {
                    return Err(HandlerError::Rejected(
                        "deposit amount must be positive".to_string(),
                    ));
                }

                let mut account = ctx
                    .repo()
                    .get(&input.id)?
                    .ok_or_else(|| HandlerError::NotFound(input.id.clone()))?;
                account.deposit(input.amount_cents)?;

                let event = MoneyDeposited {
                    account_id: input.id.clone(),
                    amount_cents: input.amount_cents,
                    new_balance_cents: account.balance_cents,
                };
                let mut outbox = OutboxMessage::encode_for_entity(
                    format!("{}:deposited:{}", input.id, account.entity.version()),
                    "MoneyDeposited",
                    &event,
                    &account.entity,
                )?;

                ctx.repo().outbox(&mut outbox).commit(&mut account)?;

                Ok(json!({
                    "id": input.id,
                    "balance_cents": account.balance_cents,
                }))
            },
        );

    Arc::new(service)
}

fn account_summary_query_service(store: HashMapRepository) -> Arc<Service<HashMapRepository>> {
    let service = Service::new(store).command_guarded(
        "account.summary.get",
        |ctx| ctx.has_fields(&["account_id"]),
        |ctx| {
            let input = ctx.input::<GetAccountSummary>()?;
            let summary = ctx
                .repo()
                .read_models::<AccountSummary>()
                .get(&input.account_id)
                .map_err(|err| HandlerError::Other(Box::new(err)))?
                .ok_or_else(|| HandlerError::NotFound(input.account_id.clone()))?;

            Ok(serde_json::to_value(summary.data)?)
        },
    );

    Arc::new(service)
}

struct ReadModelServiceHandle {
    stop_tx: mpsc::Sender<()>,
    handle: thread::JoinHandle<()>,
}

impl ReadModelServiceHandle {
    fn stop(self) {
        let _ = self.stop_tx.send(());
        self.handle
            .join()
            .expect("read model service should stop cleanly");
    }
}

fn start_account_summary_service(
    queue: InMemoryQueue,
    store: HashMapRepository,
) -> ReadModelServiceHandle {
    let (stop_tx, stop_rx) = mpsc::channel();

    let handle = thread::spawn(move || {
        let bus = Bus::from_queue(queue);
        let events = bus.subscribe(&["AccountOpened", "MoneyDeposited"]);

        loop {
            match stop_rx.try_recv() {
                Ok(()) | Err(TryRecvError::Disconnected) => break,
                Err(TryRecvError::Empty) => {}
            }

            match events.recv(10) {
                Ok(Some(event)) => {
                    project_account_summary(&store, &event);
                    events
                        .ack(&event.id)
                        .expect("read model service should ack projected events");
                }
                Ok(None) => {}
                Err(err) => panic!("read model service failed to receive event: {err}"),
            }
        }
    });

    ReadModelServiceHandle { stop_tx, handle }
}

fn load_summary(store: &HashMapRepository, account_id: &str) -> AccountSummary {
    store
        .read_models::<AccountSummary>()
        .get(account_id)
        .expect("read model load should succeed")
        .map(|view| view.data)
        .unwrap_or_else(|| AccountSummary::empty(account_id))
}

fn project_account_summary(store: &HashMapRepository, event: &Event) {
    match event.event_type.as_str() {
        "AccountOpened" => {
            let payload: AccountOpened = event.decode().expect("AccountOpened should decode");
            let mut summary = load_summary(store, &payload.account_id);
            if summary.has_projected(&event.id) {
                return;
            }

            summary.owner = Some(payload.owner);
            summary.mark_projected(&event.id);
            store
                .read_models::<AccountSummary>()
                .upsert(&summary)
                .expect("AccountOpened projection should persist");
        }
        "MoneyDeposited" => {
            let payload: MoneyDeposited = event.decode().expect("MoneyDeposited should decode");
            let mut summary = load_summary(store, &payload.account_id);
            if summary.has_projected(&event.id) {
                return;
            }

            summary.balance_cents = payload.new_balance_cents;
            summary.deposit_count += 1;
            summary.mark_projected(&event.id);
            store
                .read_models::<AccountSummary>()
                .upsert(&summary)
                .expect("MoneyDeposited projection should persist");
        }
        other => panic!("unexpected account event: {other}"),
    }
}

fn wait_for_summary(
    store: &HashMapRepository,
    account_id: &str,
    ready: impl Fn(&AccountSummary) -> bool,
) -> AccountSummary {
    let deadline = Instant::now() + Duration::from_secs(3);

    loop {
        if let Some(summary) = store
            .read_models::<AccountSummary>()
            .get(account_id)
            .expect("read model load should succeed")
            .map(|view| view.data)
        {
            if ready(&summary) {
                return summary;
            }
        }

        assert!(
            Instant::now() < deadline,
            "timed out waiting for account summary projection"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(feature = "http")]
async fn start_http_service<R: Send + Sync + 'static>(service: Arc<Service<R>>) -> String {
    let app = microsvc::router(service);
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

    let model_listener = microsvc::listen(
        model_service.clone(),
        "account-model",
        queue.clone(),
        Duration::from_millis(5),
    );
    let outbox_worker =
        OutboxWorkerThread::spawn(write_store.clone(), queue.clone(), Duration::from_millis(5));

    let read_store = HashMapRepository::new();
    let read_model_service = start_account_summary_service(queue.clone(), read_store.clone());
    let query_service = account_summary_query_service(read_store.clone());

    let client = Bus::from_queue(queue.clone());
    client
        .send(
            "account-model",
            Event::json_encode(
                "cmd-open-acct-1",
                "account.open",
                &OpenAccount {
                    id: "acct-1".to_string(),
                    owner: "Ada Lovelace".to_string(),
                },
            )
            .expect("open command should encode"),
        )
        .expect("open command should send");
    client
        .send(
            "account-model",
            Event::json_encode(
                "cmd-deposit-acct-1",
                "account.deposit",
                &DepositMoney {
                    id: "acct-1".to_string(),
                    amount_cents: 2500,
                },
            )
            .expect("deposit command should encode"),
        )
        .expect("deposit command should send");

    let summary = wait_for_summary(&read_store, "acct-1", |summary| {
        summary.owner.as_deref() == Some("Ada Lovelace")
            && summary.balance_cents == 2500
            && summary.deposit_count == 1
    });

    assert_eq!(summary.owner.as_deref(), Some("Ada Lovelace"));
    assert_eq!(summary.balance_cents, 2500);
    assert_eq!(summary.deposit_count, 1);

    let queried_summary: AccountSummary = serde_json::from_value(
        query_service
            .dispatch(
                "account.summary.get",
                json!({ "account_id": "acct-1" }),
                Session::new(),
            )
            .expect("query service should find projected account summary"),
    )
    .expect("query response should decode");
    assert_eq!(queried_summary.owner.as_deref(), Some("Ada Lovelace"));
    assert_eq!(queried_summary.balance_cents, 2500);
    assert_eq!(queried_summary.deposit_count, 1);
    assert!(matches!(
        query_service.dispatch(
            "account.summary.get",
            json!({ "account_id": "missing-account" }),
            Session::new(),
        ),
        Err(HandlerError::NotFound(id)) if id == "missing-account"
    ));

    let mut query_commands = query_service.commands();
    query_commands.sort();
    assert_eq!(
        query_commands,
        vec!["account.summary.get"],
        "query process should expose only read-model queries"
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
    let model_stats = model_listener
        .stop()
        .expect("model service listener should stop cleanly");
    let worker_stats = outbox_worker
        .stop()
        .expect("outbox worker should stop cleanly");

    assert_eq!(model_stats.handled, 2);
    assert_eq!(model_stats.failed, 0);
    assert!(worker_stats.messages_published >= 2);
}

#[cfg(feature = "http")]
#[tokio::test]
async fn model_and_read_model_query_can_be_http_services() {
    let queue = InMemoryQueue::new();

    let write_store = HashMapRepository::new();
    let account_repo = write_store.clone().queued().aggregate::<Account>();
    let model_service = account_model_service(account_repo);
    let model_base = start_http_service(model_service).await;
    let outbox_worker =
        OutboxWorkerThread::spawn(write_store.clone(), queue.clone(), Duration::from_millis(5));

    let read_store = HashMapRepository::new();
    let read_model_service = start_account_summary_service(queue.clone(), read_store.clone());
    let query_service = account_summary_query_service(read_store.clone());
    let query_base = start_http_service(query_service).await;

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

    wait_for_summary(&read_store, "acct-http", |summary| {
        summary.owner.as_deref() == Some("Grace Hopper")
            && summary.balance_cents == 4200
            && summary.deposit_count == 1
    });

    let query = client
        .post(format!("{query_base}/account.summary.get"))
        .json(&GetAccountSummary {
            account_id: "acct-http".to_string(),
        })
        .send()
        .await
        .expect("HTTP read-model service should accept query request");
    assert_eq!(query.status(), 200);

    let summary: AccountSummary = query
        .json()
        .await
        .expect("HTTP query response should decode as account summary");
    assert_eq!(summary.owner.as_deref(), Some("Grace Hopper"));
    assert_eq!(summary.balance_cents, 4200);
    assert_eq!(summary.deposit_count, 1);

    let missing = client
        .post(format!("{query_base}/account.summary.get"))
        .json(&GetAccountSummary {
            account_id: "missing-http-account".to_string(),
        })
        .send()
        .await
        .expect("HTTP read-model service should accept missing query request");
    assert_eq!(missing.status(), 404);

    read_model_service.stop();
    let worker_stats = outbox_worker
        .stop()
        .expect("outbox worker should stop cleanly");
    assert!(worker_stats.messages_published >= 2);
}

#[cfg(feature = "grpc")]
#[tokio::test]
async fn model_and_read_model_query_can_be_grpc_services() {
    let queue = InMemoryQueue::new();

    let write_store = HashMapRepository::new();
    let account_repo = write_store.clone().queued().aggregate::<Account>();
    let model_service = account_model_service(account_repo);
    let mut model_client = start_grpc_service(model_service).await;
    let outbox_worker =
        OutboxWorkerThread::spawn(write_store.clone(), queue.clone(), Duration::from_millis(5));

    let read_store = HashMapRepository::new();
    let read_model_service = start_account_summary_service(queue.clone(), read_store.clone());
    let query_service = account_summary_query_service(read_store.clone());
    let mut query_client = start_grpc_service(query_service).await;

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

    wait_for_summary(&read_store, "acct-grpc", |summary| {
        summary.owner.as_deref() == Some("Katherine Johnson")
            && summary.balance_cents == 7300
            && summary.deposit_count == 1
    });

    let query = query_client
        .dispatch(sourced_rust::microsvc::grpc::GrpcRequest {
            command: "account.summary.get".to_string(),
            input: serde_json::to_string(&GetAccountSummary {
                account_id: "acct-grpc".to_string(),
            })
            .expect("query request should encode"),
            session_variables: Default::default(),
        })
        .await
        .expect("gRPC read-model service should accept query request")
        .into_inner();
    assert_eq!(query.status, 200);

    let summary: AccountSummary =
        serde_json::from_str(&query.body).expect("gRPC query body should decode");
    assert_eq!(summary.owner.as_deref(), Some("Katherine Johnson"));
    assert_eq!(summary.balance_cents, 7300);
    assert_eq!(summary.deposit_count, 1);

    let missing = query_client
        .dispatch(sourced_rust::microsvc::grpc::GrpcRequest {
            command: "account.summary.get".to_string(),
            input: serde_json::to_string(&GetAccountSummary {
                account_id: "missing-grpc-account".to_string(),
            })
            .expect("missing query request should encode"),
            session_variables: Default::default(),
        })
        .await
        .expect("gRPC read-model service should accept missing query request")
        .into_inner();
    assert_eq!(missing.status, 404);

    read_model_service.stop();
    let worker_stats = outbox_worker
        .stop()
        .expect("outbox worker should stop cleanly");
    assert!(worker_stats.messages_published >= 2);
}
