//! Distributed read-model example over an event-driven seat checkout flow.
//!
//! Deployment shape:
//! - the **checkout saga service** owns the `CheckoutSaga` aggregate and its outbox;
//! - the **seat inventory service** owns the `Seat` aggregate and its outbox;
//! - coordinator subscribers translate domain events into aggregate method calls;
//! - the **projection service** consumes the same bus and reconciles normalized
//!   `checkouts`, `checkout_steps`, and `seats` rows in a shared read store;
//! - a **query service** reads the projected graph through primary-key loads
//!   plus `has_many` / `belongs_to` relationship includes.
//!
//! Commands are present-tense requests. Events are past-tense facts. The saga
//! is an aggregate that records checkout-process facts and emits its own events;
//! it does not directly issue commands to the seat aggregate.

mod checkout;
mod checkout_saga_service;
mod projection_service;
mod query_service;
mod read_models;
mod seat_inventory_service;

#[cfg(any(feature = "sqlite", feature = "postgres"))]
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};
#[cfg(any(feature = "sqlite", feature = "postgres"))]
use std::time::{SystemTime, UNIX_EPOCH};

use checkout::{
    checkout_command, seat_command, AddSeat, StartCheckout, CHECKOUT_SEAT_RESERVED, SEAT_RESERVED,
    SEAT_RESERVED_MESSAGE,
};
#[cfg(any(feature = "sqlite", feature = "postgres"))]
use checkout::{
    checkout_event, json_outbox_event, seat_event, CheckoutStarted, SeatAdded,
    SeatReservationCompleted, SeatReserved, CHECKOUT_STARTED, RESERVING_SEAT_MESSAGE,
    SEAT_AVAILABLE,
};
use checkout_saga_service::CheckoutSaga;
use projection_service::{service as projection_service, CHECKOUT_SCREEN_CONSUMER};
use query_service::CheckoutQueryService;
use read_models::{register_schemas, CheckoutView};
#[cfg(any(feature = "sqlite", feature = "postgres"))]
use read_models::{CheckoutStepView, SeatView};
use seat_inventory_service::Seat;
use serde::Serialize;
use sourced_rust::bus::Subscribable;
use sourced_rust::microsvc::{self, Service, Session};
#[cfg(feature = "postgres")]
use sourced_rust::PostgresRepository;
#[cfg(feature = "sqlite")]
use sourced_rust::SqliteRepository;
#[cfg(any(feature = "sqlite", feature = "postgres"))]
use sourced_rust::{
    impl_aggregate, Aggregate, AsyncAggregateBuilder, AsyncGetStream, AsyncOutboxStore,
    AsyncReadModelWritePlanCommitExt, AsyncReadModelWritePlanStore,
    AsyncRelationalReadModelQueryStore, AsyncTransactionalCommit, Entity, EventRecord,
    OutboxMessage, ReadModelError, ReadModelWritePlanBuilder, RelationalReadModel,
    RelationalReadModelIncludes, StreamIdentity,
};
use sourced_rust::{
    AggregateBuilder, HashMapRepository, InMemoryQueue, InMemoryReadModelStore, OutboxWorkerThread,
    Queueable, ReadModelWritePlanStore,
};

fn dispatch<D, C>(service: &Service<D>, command: &str, input: C)
where
    D: Send + Sync + 'static,
    C: Serialize,
{
    service
        .dispatch(
            command,
            serde_json::to_value(input).expect("command should encode"),
            Session::new(),
        )
        .unwrap_or_else(|err| panic!("{command} should dispatch: {err:?}"));
}

fn wait_for_checkout_state(
    query: &CheckoutQueryService,
    checkout_id: &str,
    ready: impl Fn(&CheckoutView) -> bool,
) -> CheckoutView {
    let deadline = Instant::now() + Duration::from_secs(10);

    loop {
        if let Some(checkout) = query
            .checkout_screen(checkout_id)
            .expect("query should succeed")
        {
            if ready(&checkout) {
                return checkout;
            }
        }

        assert!(
            Instant::now() < deadline,
            "timed out waiting for checkout {checkout_id}"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
static NEXT_ASYNC_FLOW_ID: AtomicU64 = AtomicU64::new(1);

#[cfg(any(feature = "sqlite", feature = "postgres"))]
#[derive(Default)]
struct ProjectionCheckpoint {
    entity: Entity,
    last_message_id: String,
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
impl ProjectionCheckpoint {
    fn mark_projected(&mut self, message_id: &str) {
        if self.entity.id().is_empty() {
            self.entity.set_id(CHECKOUT_SCREEN_CONSUMER);
        }
        self.last_message_id = message_id.to_string();
        self.entity
            .digest("MessageProjected", &self.last_message_id)
            .expect("projection checkpoint event should record");
    }

    fn replay(&mut self, event: &EventRecord) -> Result<(), String> {
        if event.event_name == "MessageProjected" {
            self.last_message_id = event.decode::<String>().map_err(|err| err.to_string())?;
        }
        Ok(())
    }
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
impl_aggregate!(
    ProjectionCheckpoint,
    entity,
    replay,
    aggregate_type = "distributed.checkout_projection_checkpoint"
);

#[cfg(any(feature = "sqlite", feature = "postgres"))]
struct AsyncFlowIds {
    checkout_id: String,
    seat_id: String,
    category: String,
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
fn async_unique_id(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after epoch")
        .as_nanos();
    let sequence = NEXT_ASYNC_FLOW_ID.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{nanos}-{sequence}")
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
async fn run_async_persistent_checkout_flow<R, CheckoutOutbox, SeatOutbox>(
    checkout_repo: R,
    checkout_outbox: CheckoutOutbox,
    seat_repo: R,
    seat_outbox: SeatOutbox,
    read_repo: R,
    ids: AsyncFlowIds,
) where
    R: Clone
        + AsyncGetStream
        + AsyncReadModelWritePlanStore
        + AsyncRelationalReadModelQueryStore
        + AsyncTransactionalCommit
        + Send
        + Sync
        + 'static,
    CheckoutOutbox: AsyncOutboxStore + Send + Sync,
    SeatOutbox: AsyncOutboxStore + Send + Sync,
{
    let seat_added = add_seat_async(&seat_repo, &ids.seat_id, &ids.category).await;
    assert_pending_async(&seat_outbox, &seat_added).await;
    project_message_async(&read_repo, &seat_added).await;

    let checkout_started = start_checkout_async(
        &checkout_repo,
        &ids.checkout_id,
        &ids.seat_id,
        &ids.category,
    )
    .await;
    assert_pending_async(&checkout_outbox, &checkout_started).await;
    project_message_async(&read_repo, &checkout_started).await;

    let seat_reserved = reserve_started_checkout_seat_async(&seat_repo, &checkout_started).await;
    assert_pending_async(&seat_outbox, &seat_reserved).await;
    project_message_async(&read_repo, &seat_reserved).await;

    let reservation_completed = record_seat_reserved_async(&checkout_repo, &seat_reserved).await;
    assert_pending_async(&checkout_outbox, &reservation_completed).await;
    project_message_async(&read_repo, &reservation_completed).await;

    let checkout = load_checkout_screen_async(&read_repo, &ids.checkout_id)
        .await
        .expect("checkout read model load should succeed")
        .expect("checkout should be projected");
    assert_eq!(checkout.seat_id, ids.seat_id);
    assert_eq!(checkout.seat_category, ids.category);
    assert_eq!(checkout.status, CHECKOUT_SEAT_RESERVED);
    assert_eq!(checkout.screen_message, SEAT_RESERVED_MESSAGE);
    assert_eq!(
        checkout
            .seat
            .as_ref()
            .expect("checkout should include seat")
            .status,
        SEAT_RESERVED
    );

    let mut steps: Vec<&str> = checkout
        .steps
        .iter()
        .map(|step| step.step.as_str())
        .collect();
    steps.sort();
    assert_eq!(
        steps,
        vec!["seat_reservation_completed", "seat_reserved", "started"]
    );

    let seat = load_seat_async(&read_repo, &ids.seat_id)
        .await
        .expect("seat read model load should succeed")
        .expect("seat should be projected");
    assert_eq!(seat.status, SEAT_RESERVED);
    assert_eq!(seat.checkout_id, ids.checkout_id);

    let loaded_checkout = checkout_repo
        .clone()
        .async_aggregate::<CheckoutSaga>()
        .get(&ids.checkout_id)
        .await
        .expect("checkout saga should reload")
        .expect("checkout saga should exist");
    assert_eq!(loaded_checkout.status, CHECKOUT_SEAT_RESERVED);
    assert_eq!(loaded_checkout.reserved_seat_id, ids.seat_id);

    let loaded_seat = seat_repo
        .clone()
        .async_aggregate::<Seat>()
        .get(&ids.seat_id)
        .await
        .expect("seat aggregate should reload")
        .expect("seat aggregate should exist");
    assert_eq!(loaded_seat.status, SEAT_RESERVED);
    assert_eq!(loaded_seat.checkout_id, ids.checkout_id);

    for message in [
        &seat_added,
        &checkout_started,
        &seat_reserved,
        &reservation_completed,
    ] {
        assert!(
            read_repo
                .is_processed_async(CHECKOUT_SCREEN_CONSUMER, message.id())
                .await
                .expect("processed lookup should succeed"),
            "message {} should be marked processed",
            message.id()
        );
    }

    let checkpoint_identity = StreamIdentity::new(
        ProjectionCheckpoint::aggregate_type(),
        CHECKOUT_SCREEN_CONSUMER,
    )
    .expect("checkpoint identity should build");
    let checkpoint = read_repo
        .get_stream(&checkpoint_identity)
        .await
        .expect("projection checkpoint should reload")
        .expect("projection checkpoint should exist");
    assert_eq!(checkpoint.version(), 4);
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
async fn add_seat_async<R>(repo: &R, seat_id: &str, category: &str) -> OutboxMessage
where
    R: AsyncTransactionalCommit + Send + Sync,
{
    let mut seat = Seat::default();
    seat.add(seat_id.to_string(), category.to_string())
        .expect("seat should be valid");
    let event = SeatAdded {
        seat_id: seat_id.to_string(),
        category: category.to_string(),
    };
    let outbox = json_outbox_event(seat_id, seat_event::ADDED, &event)
        .expect("seat added outbox should encode");

    sourced_rust::AsyncCommitBuilderExt::outbox(repo, outbox.clone())
        .commit(&mut seat)
        .await
        .expect("seat add should commit");
    outbox
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
async fn start_checkout_async<R>(
    repo: &R,
    checkout_id: &str,
    seat_id: &str,
    seat_category: &str,
) -> OutboxMessage
where
    R: AsyncTransactionalCommit + Send + Sync,
{
    let mut saga = CheckoutSaga::default();
    saga.start(
        checkout_id.to_string(),
        seat_id.to_string(),
        seat_category.to_string(),
    )
    .expect("checkout should be valid");
    let event = CheckoutStarted {
        checkout_id: checkout_id.to_string(),
        seat_id: seat_id.to_string(),
        seat_category: seat_category.to_string(),
    };
    let outbox = json_outbox_event(checkout_id, checkout_event::STARTED, &event)
        .expect("checkout started outbox should encode");

    sourced_rust::AsyncCommitBuilderExt::outbox(repo, outbox.clone())
        .commit(&mut saga)
        .await
        .expect("checkout start should commit");
    outbox
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
async fn reserve_started_checkout_seat_async<R>(
    repo: &R,
    checkout_started: &OutboxMessage,
) -> OutboxMessage
where
    R: Clone + AsyncGetStream + AsyncTransactionalCommit + Send + Sync + 'static,
{
    let msg: CheckoutStarted = serde_json::from_slice(&checkout_started.payload)
        .expect("checkout started payload should decode");
    let mut seat = repo
        .clone()
        .async_aggregate::<Seat>()
        .get(&msg.seat_id)
        .await
        .expect("seat should load")
        .expect("seat should exist");
    assert_eq!(seat.status, SEAT_AVAILABLE);
    assert_eq!(seat.category, msg.seat_category);

    seat.reserve(msg.checkout_id.clone())
        .expect("seat should reserve");
    let event = SeatReserved {
        checkout_id: msg.checkout_id.clone(),
        seat_id: msg.seat_id.clone(),
        seat_category: msg.seat_category.clone(),
    };
    let outbox = json_outbox_event(&msg.checkout_id, seat_event::RESERVED, &event)
        .expect("seat reserved outbox should encode");

    sourced_rust::AsyncCommitBuilderExt::outbox(repo, outbox.clone())
        .commit(&mut seat)
        .await
        .expect("seat reservation should commit");
    outbox
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
async fn record_seat_reserved_async<R>(repo: &R, seat_reserved: &OutboxMessage) -> OutboxMessage
where
    R: Clone + AsyncGetStream + AsyncTransactionalCommit + Send + Sync + 'static,
{
    let msg: SeatReserved = serde_json::from_slice(&seat_reserved.payload)
        .expect("seat reserved payload should decode");
    let mut saga = repo
        .clone()
        .async_aggregate::<CheckoutSaga>()
        .get(&msg.checkout_id)
        .await
        .expect("checkout saga should load")
        .expect("checkout saga should exist");
    assert_eq!(saga.status, CHECKOUT_STARTED);

    saga.set_reserved_seat(msg.seat_id.clone())
        .expect("checkout saga should record reserved seat");
    let event = SeatReservationCompleted {
        checkout_id: msg.checkout_id.clone(),
        seat_id: msg.seat_id.clone(),
        seat_category: msg.seat_category.clone(),
    };
    let outbox = json_outbox_event(
        &msg.checkout_id,
        checkout_event::SEAT_RESERVATION_COMPLETED,
        &event,
    )
    .expect("seat reservation completed outbox should encode");

    sourced_rust::AsyncCommitBuilderExt::outbox(repo, outbox.clone())
        .commit(&mut saga)
        .await
        .expect("checkout saga update should commit");
    outbox
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
async fn project_message_async<R>(repo: &R, message: &OutboxMessage)
where
    R: Clone
        + AsyncGetStream
        + AsyncReadModelWritePlanStore
        + AsyncTransactionalCommit
        + Send
        + Sync
        + 'static,
{
    let mut read_models = ReadModelWritePlanBuilder::new();

    match message.event_type.as_str() {
        checkout_event::STARTED => {
            let msg: CheckoutStarted =
                serde_json::from_slice(&message.payload).expect("checkout started should decode");
            let checkout = CheckoutView {
                checkout_id: msg.checkout_id.clone(),
                seat_id: msg.seat_id.clone(),
                seat_category: msg.seat_category,
                status: CHECKOUT_STARTED.to_string(),
                screen_message: RESERVING_SEAT_MESSAGE.to_string(),
                steps: Vec::new(),
                seat: None,
            };
            let step = CheckoutStepView {
                checkout_id: msg.checkout_id,
                step: "started".to_string(),
                detail: "checkout started".to_string(),
            };
            read_models
                .upsert(&checkout)
                .expect("checkout view should serialize")
                .upsert(&step)
                .expect("checkout step should serialize");
        }
        checkout_event::SEAT_RESERVATION_COMPLETED => {
            let msg: SeatReservationCompleted = serde_json::from_slice(&message.payload)
                .expect("seat reservation completed should decode");
            let checkout = CheckoutView {
                checkout_id: msg.checkout_id.clone(),
                seat_id: msg.seat_id,
                seat_category: msg.seat_category,
                status: CHECKOUT_SEAT_RESERVED.to_string(),
                screen_message: SEAT_RESERVED_MESSAGE.to_string(),
                steps: Vec::new(),
                seat: None,
            };
            let step = CheckoutStepView {
                checkout_id: msg.checkout_id,
                step: "seat_reservation_completed".to_string(),
                detail: "seat reservation completed".to_string(),
            };
            read_models
                .upsert(&checkout)
                .expect("checkout view should serialize")
                .upsert(&step)
                .expect("checkout step should serialize");
        }
        seat_event::ADDED => {
            let msg: SeatAdded =
                serde_json::from_slice(&message.payload).expect("seat added should decode");
            let seat = SeatView {
                seat_id: msg.seat_id,
                category: msg.category,
                status: SEAT_AVAILABLE.to_string(),
                checkout_id: String::new(),
            };
            read_models
                .upsert(&seat)
                .expect("seat view should serialize");
        }
        seat_event::RESERVED => {
            let msg: SeatReserved =
                serde_json::from_slice(&message.payload).expect("seat reserved should decode");
            let seat = SeatView {
                seat_id: msg.seat_id.clone(),
                category: msg.seat_category,
                status: SEAT_RESERVED.to_string(),
                checkout_id: msg.checkout_id.clone(),
            };
            let step = CheckoutStepView {
                checkout_id: msg.checkout_id,
                step: "seat_reserved".to_string(),
                detail: "seat reserved".to_string(),
            };
            read_models
                .upsert(&seat)
                .expect("seat view should serialize")
                .upsert(&step)
                .expect("checkout step should serialize");
        }
        other => panic!("unexpected projected event type {other}"),
    }

    read_models.mark_processed(CHECKOUT_SCREEN_CONSUMER, message.id());
    let mut checkpoint = repo
        .clone()
        .async_aggregate::<ProjectionCheckpoint>()
        .get(CHECKOUT_SCREEN_CONSUMER)
        .await
        .expect("projection checkpoint should load")
        .unwrap_or_default();
    checkpoint.mark_projected(message.id());

    AsyncReadModelWritePlanCommitExt::read_models(repo, read_models)
        .commit(&mut checkpoint)
        .await
        .expect("projection read models should commit with checkpoint");
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
async fn assert_pending_async<S>(store: &S, message: &OutboxMessage)
where
    S: AsyncOutboxStore + Send + Sync,
{
    let pending = store
        .pending_async()
        .await
        .expect("pending outbox messages should load");
    assert!(
        pending.iter().any(|pending| pending.id == message.id),
        "message {} should be pending in outbox",
        message.id
    );
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
async fn load_checkout_screen_async<R>(
    repo: &R,
    checkout_id: &str,
) -> Result<Option<CheckoutView>, ReadModelError>
where
    R: AsyncRelationalReadModelQueryStore + Send + Sync,
{
    let request = ReadModelWritePlanBuilder::new().load_with::<CheckoutView, _, _>(
        read_models::checkout_key(checkout_id),
        ["steps", "seat"],
    )?;
    let graph = repo.load_graph_async(request).await?;
    let Some(root) = graph.root else {
        return Ok(None);
    };

    let mut checkout = CheckoutView::from_row(root.data)?;
    for (include_name, include_rows) in graph.includes {
        let rows = include_rows
            .rows
            .into_iter()
            .map(|row| row.data)
            .collect::<Vec<_>>();
        checkout.hydrate_include(&include_name, rows)?;
    }
    Ok(Some(checkout))
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
async fn load_seat_async<R>(repo: &R, seat_id: &str) -> Result<Option<SeatView>, ReadModelError>
where
    R: AsyncRelationalReadModelQueryStore + Send + Sync,
{
    let request =
        ReadModelWritePlanBuilder::new().load::<SeatView>(read_models::seat_key(seat_id))?;
    let graph = repo.load_graph_async(request).await?;
    Ok(graph
        .root
        .map(|root| SeatView::from_row(root.data).expect("seat row should hydrate")))
}

#[test]
fn seat_checkout_saga_reserves_seat_and_projects_user_screen() {
    let queue = InMemoryQueue::new();
    let poll = Duration::from_millis(5);

    let checkout_store = HashMapRepository::new();
    let checkout_service =
        checkout_saga_service::service(checkout_store.clone().queued().aggregate());
    let checkout_worker =
        OutboxWorkerThread::spawn(checkout_store.outbox_store(), queue.clone(), poll);
    let checkout_sub = microsvc::subscribe(checkout_service.clone(), queue.new_subscriber(), poll);

    let seat_store = HashMapRepository::new();
    let seat_service = seat_inventory_service::service(seat_store.clone().queued().aggregate());
    let seat_worker = OutboxWorkerThread::spawn(seat_store.outbox_store(), queue.clone(), poll);
    let seat_sub = microsvc::subscribe(seat_service.clone(), queue.new_subscriber(), poll);

    let read_store = InMemoryReadModelStore::new();
    register_schemas(&read_store).expect("relational schemas should register");
    let projection_svc = projection_service(read_store.clone());
    let projection_sub = microsvc::subscribe(projection_svc.clone(), queue.new_subscriber(), poll);
    let query_service = CheckoutQueryService::new(read_store.clone());

    dispatch(
        &seat_service,
        seat_command::ADD,
        AddSeat {
            seat_id: "A-7".to_string(),
            category: "balcony".to_string(),
        },
    );

    dispatch(
        &checkout_service,
        checkout_command::START,
        StartCheckout {
            checkout_id: "checkout-1".to_string(),
            seat_id: "A-7".to_string(),
            seat_category: "balcony".to_string(),
        },
    );

    let checkout = wait_for_checkout_state(&query_service, "checkout-1", |checkout| {
        checkout.status == CHECKOUT_SEAT_RESERVED
            && checkout
                .seat
                .as_ref()
                .is_some_and(|seat| seat.status == SEAT_RESERVED)
    });

    assert_eq!(checkout.seat_id, "A-7");
    assert_eq!(checkout.seat_category, "balcony");
    assert_eq!(checkout.screen_message, SEAT_RESERVED_MESSAGE);
    assert_eq!(
        checkout
            .seat
            .as_ref()
            .expect("checkout should include seat")
            .status,
        SEAT_RESERVED
    );

    let mut steps: Vec<&str> = checkout
        .steps
        .iter()
        .map(|step| step.step.as_str())
        .collect();
    steps.sort();
    assert_eq!(
        steps,
        vec!["seat_reservation_completed", "seat_reserved", "started"]
    );

    let seat = query_service
        .seat("A-7")
        .expect("seat query should succeed")
        .expect("seat should be projected");
    assert_eq!(seat.status, SEAT_RESERVED);
    assert_eq!(seat.checkout_id, "checkout-1");

    let checkout_saga = checkout_store
        .clone()
        .queued()
        .aggregate::<CheckoutSaga>()
        .peek("checkout-1")
        .unwrap()
        .unwrap();
    assert_eq!(checkout_saga.status, CHECKOUT_SEAT_RESERVED);
    assert_eq!(checkout_saga.reserved_seat_id, "A-7");

    let seat = seat_store
        .clone()
        .queued()
        .aggregate::<Seat>()
        .peek("A-7")
        .unwrap()
        .unwrap();
    assert_eq!(seat.status, SEAT_RESERVED);
    assert_eq!(seat.checkout_id, "checkout-1");

    for event in queue.events() {
        if projection_service::projects(event.event_type.as_str()) {
            assert!(
                read_store
                    .is_processed(CHECKOUT_SCREEN_CONSUMER, &event.id)
                    .expect("processed lookup should succeed"),
                "event {} should be marked processed before ack",
                event.id
            );
        }
    }

    let _ = checkout_sub.stop();
    let _ = seat_sub.stop();
    let _ = projection_sub.stop();
    let _ = checkout_worker.stop();
    let _ = seat_worker.stop();
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn async_sqlite_checkout_flow_projects_relational_read_models() {
    let checkout_repo = SqliteRepository::connect_and_migrate("sqlite::memory:")
        .await
        .expect("checkout repository should migrate");
    let checkout_outbox = checkout_repo.outbox_store();
    let seat_repo = SqliteRepository::connect_and_migrate("sqlite::memory:")
        .await
        .expect("seat repository should migrate");
    let seat_outbox = seat_repo.outbox_store();
    let read_repo = SqliteRepository::connect_and_migrate("sqlite::memory:")
        .await
        .expect("read repository should migrate");
    let registry = read_models::table_schema_registry().expect("schemas should build");
    read_repo
        .bootstrap_table_schema_for_dev(&registry)
        .await
        .expect("read schema should bootstrap");

    run_async_persistent_checkout_flow(
        checkout_repo,
        checkout_outbox,
        seat_repo,
        seat_outbox,
        read_repo,
        AsyncFlowIds {
            checkout_id: async_unique_id("checkout-sqlite"),
            seat_id: async_unique_id("seat-sqlite"),
            category: "balcony".to_string(),
        },
    )
    .await;
}

#[cfg(feature = "postgres")]
#[tokio::test]
async fn async_postgres_checkout_flow_projects_relational_read_models() {
    let Ok(database_url) = std::env::var("DATABASE_URL") else {
        eprintln!("skipping Postgres distributed read-model test: DATABASE_URL is not set");
        return;
    };

    let checkout_repo = PostgresRepository::connect_and_migrate(&database_url)
        .await
        .expect("checkout repository should migrate");
    let checkout_outbox = checkout_repo.outbox_store();
    let seat_repo = PostgresRepository::connect_and_migrate(&database_url)
        .await
        .expect("seat repository should migrate");
    let seat_outbox = seat_repo.outbox_store();
    let read_repo = PostgresRepository::connect_and_migrate(&database_url)
        .await
        .expect("read repository should migrate");
    let registry = read_models::table_schema_registry().expect("schemas should build");
    read_repo
        .bootstrap_table_schema_for_dev(&registry)
        .await
        .expect("read schema should bootstrap");

    run_async_persistent_checkout_flow(
        checkout_repo,
        checkout_outbox,
        seat_repo,
        seat_outbox,
        read_repo,
        AsyncFlowIds {
            checkout_id: async_unique_id("checkout-postgres"),
            seat_id: async_unique_id("seat-postgres"),
            category: "balcony".to_string(),
        },
    )
    .await;
}

#[cfg(feature = "http")]
#[tokio::test]
async fn checkout_commands_can_be_http_service() {
    let checkout_store = HashMapRepository::new();
    let checkout_service =
        checkout_saga_service::service(checkout_store.clone().queued().aggregate());
    let base = checkout_saga_service::start_http_service(checkout_service.clone()).await;

    let client = reqwest::Client::new();
    let started = client
        .post(format!("{base}/{}", checkout_command::START))
        .json(&StartCheckout {
            checkout_id: "checkout-http".to_string(),
            seat_id: "B-2".to_string(),
            seat_category: "floor".to_string(),
        })
        .send()
        .await
        .expect("HTTP checkout service should accept start request");
    assert_eq!(started.status(), 200);

    let saga = checkout_service
        .repo()
        .peek("checkout-http")
        .expect("HTTP write-side load should succeed")
        .expect("HTTP write-side checkout should exist");
    assert_eq!(saga.status, checkout::CHECKOUT_STARTED);
}

#[cfg(feature = "grpc")]
#[tokio::test]
async fn checkout_commands_can_be_grpc_service() {
    let checkout_store = HashMapRepository::new();
    let checkout_service =
        checkout_saga_service::service(checkout_store.clone().queued().aggregate());
    let mut client = checkout_saga_service::start_grpc_service(checkout_service.clone()).await;

    let started = client
        .dispatch(sourced_rust::microsvc::grpc::GrpcRequest {
            command: checkout_command::START.to_string(),
            input: serde_json::to_string(&StartCheckout {
                checkout_id: "checkout-grpc".to_string(),
                seat_id: "C-4".to_string(),
                seat_category: "box".to_string(),
            })
            .expect("start command should encode"),
            session_variables: Default::default(),
        })
        .await
        .expect("gRPC checkout service should accept start request")
        .into_inner();
    assert_eq!(started.status, 200);

    let saga = checkout_service
        .repo()
        .peek("checkout-grpc")
        .expect("gRPC write-side load should succeed")
        .expect("gRPC write-side checkout should exist");
    assert_eq!(saga.status, checkout::CHECKOUT_STARTED);
}
