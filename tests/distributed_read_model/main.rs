//! Distributed read-model example over normalized relational tables.
//!
//! Deployment shape:
//! - the **catalog service** owns the `Product` aggregate and its outbox;
//! - the **order service** owns the `Order` aggregate (with line items as
//!   aggregate state) and its outbox;
//! - two **projection services** consume the bus and reconcile normalized
//!   `products`, `orders`, and `order_lines` rows in a shared read store;
//! - a **query service** reads the projected graph through primary-key loads
//!   plus `has_many` / `belongs_to` relationship includes.
//!
//! The write services share nothing with the read side except the bus. The
//! order projector relies on `save_changes` collection sync: each order snapshot
//! is the desired state, so a removed line becomes a deleted `order_lines` row.
//! Threads and `InMemoryQueue` stand in for separate processes and a broker; in
//! production a query gateway such as Hasura would sit in front of the tables.

mod catalog_service;
mod fulfillment;
mod inventory_service;
mod order_fulfillment_saga_service;
mod order_service;
mod payment_service;
mod projections_service;
mod query_service;
mod read_models;

use std::thread;
use std::time::{Duration, Instant};

use catalog_service::AddProduct;
use fulfillment::{requested_event, FulfillmentMsg};
use inventory_service::Inventory;
use order_fulfillment_saga_service::OrderFulfillmentSaga;
use order_service::{AddLine, ChangeQuantity, PlaceOrder, RemoveLine, SubmitOrder};
use payment_service::Payment;
use projections_service::{
    start_projection_service, CATALOG_CONSUMER, FULFILLMENT_CONSUMER, ORDER_CONSUMER,
};
use query_service::OrderQueryService;
use read_models::{register_schemas, OrderView};
use serde::Serialize;
use sourced_rust::bus::{Bus, Subscribable};
use sourced_rust::microsvc::{self, Service, Session};
use sourced_rust::{
    AggregateBuilder, HashMapRepository, InMemoryQueue, InMemoryReadModelStore, OutboxWorkerThread,
    Queueable, ReadModelSessionStore,
};

fn dispatch<R, C>(service: &Service<R>, command: &str, input: C)
where
    R: Send + Sync + 'static,
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

fn wait_for_order_state(
    query: &OrderQueryService,
    order_id: &str,
    ready: impl Fn(&OrderView) -> bool,
) -> OrderView {
    let deadline = Instant::now() + Duration::from_secs(10);

    loop {
        if let Some(order) = query
            .order_with_lines_and_steps(order_id)
            .expect("query should succeed")
        {
            if ready(&order) {
                return order;
            }
        }

        assert!(
            Instant::now() < deadline,
            "timed out waiting for order {order_id}"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn catalog_and_order_services_feed_a_normalized_read_model() {
    let queue = InMemoryQueue::new();

    // Two independent write services, each with its own event store and outbox.
    let catalog_store = HashMapRepository::new();
    let catalog_service =
        catalog_service::model_service(catalog_store.clone().queued().aggregate());
    let order_store = HashMapRepository::new();
    let order_service = order_service::model_service(order_store.clone().queued().aggregate());

    let catalog_worker = OutboxWorkerThread::spawn(
        catalog_store.clone(),
        queue.clone(),
        Duration::from_millis(5),
    );
    let order_worker =
        OutboxWorkerThread::spawn(order_store.clone(), queue.clone(), Duration::from_millis(5));

    // Shared downstream read store with normalized relational tables.
    let read_store = InMemoryReadModelStore::new();
    register_schemas(&read_store).expect("relational schemas should register");
    let projection = start_projection_service(queue.clone(), read_store.clone());
    let query_service = OrderQueryService::new(read_store.clone());

    // Saga subsystem: inventory, payment, and the orchestrator are ordinary
    // `microsvc::Service`s. Each publishes via its outbox worker and subscribes
    // to the bus with `microsvc::subscribe`, dispatching events to handlers by
    // type — the same shape as every other service. The order service also
    // subscribes, so it reacts to the saga's confirm/cancel decisions.
    let poll = Duration::from_millis(5);

    let inventory_store = HashMapRepository::new();
    inventory_service::seed_stock(&inventory_store, "W", 100);
    let inventory_svc =
        inventory_service::model_service(inventory_store.clone().queued().aggregate());
    let inventory_worker = OutboxWorkerThread::spawn(inventory_store.clone(), queue.clone(), poll);
    let inventory_sub = microsvc::subscribe(inventory_svc.clone(), queue.new_subscriber(), poll);

    let payment_store = HashMapRepository::new();
    let payment_svc = payment_service::model_service(payment_store.clone().queued().aggregate());
    let payment_worker = OutboxWorkerThread::spawn(payment_store.clone(), queue.clone(), poll);
    let payment_sub = microsvc::subscribe(payment_svc.clone(), queue.new_subscriber(), poll);

    let saga_store = HashMapRepository::new();
    let saga_svc =
        order_fulfillment_saga_service::model_service(saga_store.clone().queued().aggregate());
    let saga_worker = OutboxWorkerThread::spawn(saga_store.clone(), queue.clone(), poll);
    let saga_sub = microsvc::subscribe(saga_svc.clone(), queue.new_subscriber(), poll);

    let order_sub = microsvc::subscribe(order_service.clone(), queue.new_subscriber(), poll);

    // Catalog commands.
    dispatch(
        &catalog_service,
        "product.add",
        AddProduct {
            id: "prod-widget".to_string(),
            name: "Widget".to_string(),
            unit_cents: 500,
        },
    );
    dispatch(
        &catalog_service,
        "product.add",
        AddProduct {
            id: "prod-gadget".to_string(),
            name: "Gadget".to_string(),
            unit_cents: 1000,
        },
    );

    // Order commands: place, add two lines, change one, remove one, submit.
    dispatch(
        &order_service,
        "order.place",
        PlaceOrder {
            id: "order-1".to_string(),
            customer: "Ada Lovelace".to_string(),
        },
    );
    dispatch(
        &order_service,
        "order.add_line",
        AddLine {
            id: "order-1".to_string(),
            sku: "W".to_string(),
            product_id: "prod-widget".to_string(),
            unit_cents: 500,
            quantity: 2,
        },
    );
    dispatch(
        &order_service,
        "order.add_line",
        AddLine {
            id: "order-1".to_string(),
            sku: "G".to_string(),
            product_id: "prod-gadget".to_string(),
            unit_cents: 1000,
            quantity: 1,
        },
    );
    dispatch(
        &order_service,
        "order.change_quantity",
        ChangeQuantity {
            id: "order-1".to_string(),
            sku: "W".to_string(),
            quantity: 3,
        },
    );
    dispatch(
        &order_service,
        "order.remove_line",
        RemoveLine {
            id: "order-1".to_string(),
            sku: "G".to_string(),
        },
    );
    dispatch(
        &order_service,
        "order.submit",
        SubmitOrder {
            id: "order-1".to_string(),
        },
    );

    // Kick off fulfillment for the happy order (amount within the payment cap).
    Bus::from_queue(queue.clone())
        .publish(requested_event(&FulfillmentMsg {
            order_id: "order-1".to_string(),
            sku: "W".to_string(),
            quantity: 3,
            amount_cents: 1500,
            ..Default::default()
        }))
        .expect("fulfillment kickoff should publish");

    // A second, expensive order the payment service declines, exercising the
    // compensation path (release inventory, cancel the order).
    dispatch(
        &order_service,
        "order.place",
        PlaceOrder {
            id: "order-2".to_string(),
            customer: "Grace Hopper".to_string(),
        },
    );
    dispatch(
        &order_service,
        "order.add_line",
        AddLine {
            id: "order-2".to_string(),
            sku: "W".to_string(),
            product_id: "prod-widget".to_string(),
            unit_cents: 200_000,
            quantity: 1,
        },
    );
    dispatch(
        &order_service,
        "order.submit",
        SubmitOrder {
            id: "order-2".to_string(),
        },
    );
    Bus::from_queue(queue.clone())
        .publish(requested_event(&FulfillmentMsg {
            order_id: "order-2".to_string(),
            sku: "W".to_string(),
            quantity: 1,
            amount_cents: 200_000,
            ..Default::default()
        }))
        .expect("fulfillment kickoff should publish");

    // === Happy path: the saga drives order-1 to confirmed ===
    let order = wait_for_order_state(&query_service, "order-1", |order| {
        order.status == "confirmed" && order.fulfillment_steps.len() == 3
    });
    assert_eq!(order.customer, "Ada Lovelace");
    assert_eq!(order.total_cents, 1500);
    assert_eq!(order.lines.len(), 1, "removed line should be deleted");
    assert_eq!(order.lines[0].sku, "W");
    assert_eq!(order.lines[0].quantity, 3);
    // JSONB column round-trips structured data alongside scalar columns.
    assert_eq!(
        order.metadata.get("source").map(String::as_str),
        Some("order-service")
    );
    // The saga audit trail is a has_many child, projected from saga events.
    let mut steps: Vec<&str> = order
        .fulfillment_steps
        .iter()
        .map(|step| step.step.as_str())
        .collect();
    steps.sort();
    assert_eq!(
        steps,
        vec!["inventory_reserved", "payment_succeeded", "requested"]
    );

    // belongs_to include joins the line to its catalog product (cross-service).
    let line = query_service
        .line_with_product("order-1", "W")
        .expect("query should succeed")
        .expect("line should exist");
    let product = line.product.expect("belongs_to product should hydrate");
    assert_eq!(product.name, "Widget");

    // === Compensation path: the saga cancels order-2 ===
    let cancelled = wait_for_order_state(&query_service, "order-2", |order| {
        order.status == "cancelled" && order.fulfillment_steps.len() == 4
    });
    let mut comp_steps: Vec<&str> = cancelled
        .fulfillment_steps
        .iter()
        .map(|step| step.step.as_str())
        .collect();
    comp_steps.sort();
    assert_eq!(
        comp_steps,
        vec![
            "inventory_released",
            "inventory_reserved",
            "payment_declined",
            "requested",
        ]
    );

    // Write-side aggregates reflect the saga outcomes.
    assert_eq!(
        order_service
            .repo()
            .peek("order-1")
            .unwrap()
            .unwrap()
            .status,
        "confirmed"
    );
    assert_eq!(
        order_service
            .repo()
            .peek("order-2")
            .unwrap()
            .unwrap()
            .status,
        "cancelled"
    );
    let saga_repo = saga_store
        .clone()
        .queued()
        .aggregate::<OrderFulfillmentSaga>();
    assert_eq!(
        saga_repo.peek("order-1").unwrap().unwrap().status,
        "completed"
    );
    assert_eq!(
        saga_repo.peek("order-2").unwrap().unwrap().status,
        "cancelled"
    );

    // Inventory: order-1 holds 3 reserved; order-2's reservation was released.
    let inventory = inventory_store
        .clone()
        .queued()
        .aggregate::<Inventory>()
        .peek("W")
        .unwrap()
        .unwrap();
    assert_eq!(inventory.available, 97);
    assert_eq!(inventory.reserved, 3);

    // Payment: order-1 charged, order-2 declined.
    let payment_repo = payment_store.clone().queued().aggregate::<Payment>();
    assert_eq!(
        payment_repo.peek("order-1").unwrap().unwrap().status,
        "charged"
    );
    assert!(payment_repo
        .peek("order-2")
        .unwrap()
        .unwrap()
        .status
        .starts_with("declined"));

    // Idempotency: every projected event is marked processed by its consumer.
    for event in queue.events() {
        let event_type = event.event_type.as_str();
        let consumer = if event_type.starts_with("product.") {
            CATALOG_CONSUMER
        } else if event_type.starts_with("order.") {
            ORDER_CONSUMER
        } else if matches!(
            event_type,
            "fulfillment.requested"
                | "fulfillment.inventory_reserved"
                | "fulfillment.payment_succeeded"
                | "fulfillment.payment_declined"
                | "fulfillment.inventory_released"
        ) {
            FULFILLMENT_CONSUMER
        } else {
            continue;
        };
        assert!(
            read_store
                .is_processed(consumer, &event.id)
                .expect("processed lookup should succeed"),
            "event {} should be marked processed before ack",
            event.id
        );
    }

    let _ = order_sub.stop();
    let _ = saga_sub.stop();
    let _ = payment_sub.stop();
    let _ = inventory_sub.stop();
    projection.stop();
    let _ = saga_worker.stop();
    let _ = payment_worker.stop();
    let _ = inventory_worker.stop();
    let _ = order_worker.stop();
    let _ = catalog_worker.stop();
}

#[cfg(feature = "http")]
#[tokio::test]
async fn order_commands_can_be_http_service() {
    let order_store = HashMapRepository::new();
    let order_service = order_service::model_service(order_store.clone().queued().aggregate());
    let base = order_service::start_http_service(order_service.clone()).await;

    let client = reqwest::Client::new();
    let placed = client
        .post(format!("{base}/order.place"))
        .json(&PlaceOrder {
            id: "order-http".to_string(),
            customer: "Grace Hopper".to_string(),
        })
        .send()
        .await
        .expect("HTTP order service should accept place request");
    assert_eq!(placed.status(), 200);

    let order = order_service
        .repo()
        .peek("order-http")
        .expect("HTTP write-side load should succeed")
        .expect("HTTP write-side order should exist");
    assert_eq!(order.status, "open");
}

#[cfg(feature = "grpc")]
#[tokio::test]
async fn order_commands_can_be_grpc_service() {
    let order_store = HashMapRepository::new();
    let order_service = order_service::model_service(order_store.clone().queued().aggregate());
    let mut client = order_service::start_grpc_service(order_service.clone()).await;

    let placed = client
        .dispatch(sourced_rust::microsvc::grpc::GrpcRequest {
            command: "order.place".to_string(),
            input: serde_json::to_string(&PlaceOrder {
                id: "order-grpc".to_string(),
                customer: "Katherine Johnson".to_string(),
            })
            .expect("place command should encode"),
            session_variables: Default::default(),
        })
        .await
        .expect("gRPC order service should accept place request")
        .into_inner();
    assert_eq!(placed.status, 200);

    let order = order_service
        .repo()
        .peek("order-grpc")
        .expect("gRPC write-side load should succeed")
        .expect("gRPC write-side order should exist");
    assert_eq!(order.status, "open");
}
