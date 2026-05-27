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

use std::thread;
use std::time::{Duration, Instant};

use checkout::{
    checkout_command, seat_command, AddSeat, StartCheckout, CHECKOUT_SEAT_RESERVED, SEAT_RESERVED,
    SEAT_RESERVED_MESSAGE,
};
use checkout_saga_service::CheckoutSaga;
use projection_service::{service as projection_service, CHECKOUT_SCREEN_CONSUMER};
use query_service::CheckoutQueryService;
use read_models::{register_schemas, CheckoutView};
use seat_inventory_service::Seat;
use serde::Serialize;
use sourced_rust::bus::Subscribable;
use sourced_rust::microsvc::{self, Service, Session};
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
