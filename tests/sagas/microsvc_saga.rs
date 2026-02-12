//! Microservice Saga Tests
//!
//! Demonstrates the `register_handlers!` convention with handler files
//! organized by service domain under `handlers/`.
//!
//! Each service is typed to a specific aggregate via
//! `Service::new(repo.queued().aggregate::<T>())`, so handlers access
//! `ctx.repo().get()`, `ctx.repo().commit()`, etc. directly.
//!
//! Two tests:
//! 1. **Orchestrated** — test runner dispatches commands to each service
//! 2. **Distributed** — services communicate via bus with `microsvc::listen`

use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::json;

use sourced_rust::microsvc::{self, Service, Session};
use sourced_rust::{
    AggregateBuilder, HashMapRepository, InMemoryQueue, OutboxWorkerThread, Queueable,
};

use super::handlers;
use super::order::{
    Inventory, Order, OrderFulfillmentSaga, OrderStatus, Payment, SagaStatus,
};

// ============================================================================
// Test 1: Orchestrated — test runner dispatches to each service in sequence
// ============================================================================

/// Test runner plays the role of the message bus, dispatching each command
/// in the correct order.  Outbox messages are created but ignored (no workers).
///
/// ```text
///  ┌────────────────────────────────────────────────────────────────┐
///  │                    Test Runner (orchestrator)                   │
///  │                                                                │
///  │  dispatch ──→ [Saga Service]      typed: OrderFulfillmentSaga  │
///  │  dispatch ──→ [Order Service]     typed: Order                 │
///  │  dispatch ──→ [Inventory Service] typed: Inventory             │
///  │  dispatch ──→ [Payment Service]   typed: Payment               │
///  └────────────────────────────────────────────────────────────────┘
/// ```
#[test]
fn saga_orchestrated() {
    let saga_svc = sourced_rust::register_handlers!(
        Service::new(
            HashMapRepository::new()
                .queued()
                .aggregate::<OrderFulfillmentSaga>()
        ),
        handlers::saga::start,
        handlers::saga::on_order_created,
        handlers::saga::on_inventory_reserved,
        handlers::saga::on_payment_succeeded,
        handlers::saga::on_order_completed,
    );

    let order_svc = sourced_rust::register_handlers!(
        Service::new(HashMapRepository::new().queued().aggregate::<Order>()),
        handlers::orders::create,
        handlers::orders::complete,
    );

    let inventory_svc = sourced_rust::register_handlers!(
        Service::new(HashMapRepository::new().queued().aggregate::<Inventory>()),
        handlers::inventory::init,
        handlers::inventory::reserve,
    );

    let payment_svc = sourced_rust::register_handlers!(
        Service::new(HashMapRepository::new().queued().aggregate::<Payment>()),
        handlers::payments::process,
    );

    let s = Session::new;

    // 1. Initialize inventory
    inventory_svc
        .dispatch(
            "InitInventory",
            json!({ "sku": "WIDGET-001", "stock": 100 }),
            s(),
        )
        .unwrap();

    // 2. Start saga → creates saga + outbox(CreateOrder)
    saga_svc
        .dispatch(
            "StartSaga",
            json!({
                "saga_id": "saga-001",
                "order_id": "order-001",
                "customer_id": "customer-001",
                "items": [{ "sku": "WIDGET-001", "quantity": 5, "price_cents": 1000 }],
                "total_cents": 5000
            }),
            s(),
        )
        .unwrap();

    // 3. Create order → outbox(OrderCreated)
    order_svc
        .dispatch(
            "CreateOrder",
            json!({
                "saga_id": "saga-001",
                "order_id": "order-001",
                "customer_id": "customer-001",
                "items": [{ "sku": "WIDGET-001", "quantity": 5, "price_cents": 1000 }],
                "total_cents": 5000
            }),
            s(),
        )
        .unwrap();

    // 4. Saga: order created → outbox(ReserveInventory)
    saga_svc
        .dispatch(
            "OrderCreated",
            json!({ "saga_id": "saga-001", "order_id": "order-001" }),
            s(),
        )
        .unwrap();

    // 5. Reserve inventory → outbox(InventoryReserved)
    inventory_svc
        .dispatch(
            "ReserveInventory",
            json!({
                "saga_id": "saga-001",
                "order_id": "order-001",
                "sku": "WIDGET-001",
                "quantity": 5
            }),
            s(),
        )
        .unwrap();

    // 6. Saga: inventory reserved → outbox(ProcessPayment)
    saga_svc
        .dispatch(
            "InventoryReserved",
            json!({ "saga_id": "saga-001", "order_id": "order-001" }),
            s(),
        )
        .unwrap();

    // 7. Process payment → outbox(PaymentSucceeded)
    payment_svc
        .dispatch(
            "ProcessPayment",
            json!({
                "saga_id": "saga-001",
                "order_id": "order-001",
                "amount_cents": 5000
            }),
            s(),
        )
        .unwrap();

    // 8. Saga: payment succeeded → outbox(CompleteOrder)
    saga_svc
        .dispatch(
            "PaymentSucceeded",
            json!({ "saga_id": "saga-001", "order_id": "order-001" }),
            s(),
        )
        .unwrap();

    // 9. Complete order → outbox(OrderCompleted)
    order_svc
        .dispatch(
            "CompleteOrder",
            json!({ "saga_id": "saga-001", "order_id": "order-001" }),
            s(),
        )
        .unwrap();

    // 10. Saga: order completed → saga done
    saga_svc
        .dispatch(
            "OrderCompleted",
            json!({ "saga_id": "saga-001", "order_id": "order-001" }),
            s(),
        )
        .unwrap();

    // === Verify final state — typed repos return aggregates directly ===

    let saga = saga_svc.repo().peek("saga-001").unwrap().unwrap();
    assert_eq!(saga.status(), SagaStatus::Completed);
    assert!(saga.is_complete());

    let order = order_svc.repo().peek("order-001").unwrap().unwrap();
    assert_eq!(order.status(), OrderStatus::Completed);

    let inv = inventory_svc.repo().peek("WIDGET-001").unwrap().unwrap();
    assert_eq!(inv.available(), 95);

    let payment = payment_svc.repo().peek("pay-order-001").unwrap().unwrap();
    assert!(payment.is_successful());
}

// ============================================================================
// Test 2: Distributed — services communicate via bus transport
// ============================================================================

/// Each service runs on its own named queue with an outbox worker routing
/// messages.  The test dispatches `StartSaga` and then polls for completion.
///
/// ```text
///  ┌──────────────────────────────────────────────────────────────┐
///  │              Shared Queue (InMemoryQueue)                     │
///  │  "saga"         "orders"       "inventory"     "payments"    │
///  └──────────────────────────────────────────────────────────────┘
///      ↑↓              ↑↓              ↑↓              ↑↓
///  ┌──────────┐  ┌──────────┐  ┌──────────────┐  ┌──────────┐
///  │  Saga    │  │  Order   │  │  Inventory   │  │  Payment │
///  │  Service │  │  Service │  │  Service     │  │  Service │
///  └──────────┘  └──────────┘  └──────────────┘  └──────────┘
/// ```
///
/// Flow:
/// 1. Saga starts → sends CreateOrder to "orders"
/// 2. Order creates → sends OrderCreated to "saga"
/// 3. Saga → sends ReserveInventory to "inventory"
/// 4. Inventory reserves → sends InventoryReserved to "saga"
/// 5. Saga → sends ProcessPayment to "payments"
/// 6. Payment captures → sends PaymentSucceeded to "saga"
/// 7. Saga → sends CompleteOrder to "orders"
/// 8. Order completes → sends OrderCompleted to "saga"
/// 9. Saga completes
#[test]
fn saga_distributed() {
    let queue = InMemoryQueue::new();
    let poll = Duration::from_millis(10);

    // === SAGA SERVICE ===
    let saga_repo = HashMapRepository::new();
    let saga_worker = OutboxWorkerThread::spawn_routed(saga_repo.clone(), queue.clone(), poll);
    let saga_svc = Arc::new(sourced_rust::register_handlers!(
        Service::new(saga_repo.queued().aggregate::<OrderFulfillmentSaga>()),
        handlers::saga::start,
        handlers::saga::on_order_created,
        handlers::saga::on_inventory_reserved,
        handlers::saga::on_payment_succeeded,
        handlers::saga::on_order_completed,
    ));
    let saga_listen = microsvc::listen(saga_svc.clone(), "saga", queue.clone(), poll);

    // === ORDER SERVICE ===
    let order_repo = HashMapRepository::new();
    let order_worker = OutboxWorkerThread::spawn_routed(order_repo.clone(), queue.clone(), poll);
    let order_svc = Arc::new(sourced_rust::register_handlers!(
        Service::new(order_repo.queued().aggregate::<Order>()),
        handlers::orders::create,
        handlers::orders::complete,
    ));
    let order_listen = microsvc::listen(order_svc.clone(), "orders", queue.clone(), poll);

    // === INVENTORY SERVICE ===
    let inventory_repo = HashMapRepository::new();
    let inventory_worker =
        OutboxWorkerThread::spawn_routed(inventory_repo.clone(), queue.clone(), poll);

    // Pre-seed inventory before starting the service
    {
        let tmp = inventory_repo.clone().aggregate::<Inventory>();
        let mut inv = Inventory::new();
        inv.initialize("WIDGET-001".to_string(), 100);
        tmp.commit(&mut inv).unwrap();
    }

    let inventory_svc = Arc::new(sourced_rust::register_handlers!(
        Service::new(inventory_repo.queued().aggregate::<Inventory>()),
        handlers::inventory::init,
        handlers::inventory::reserve,
    ));
    let inventory_listen =
        microsvc::listen(inventory_svc.clone(), "inventory", queue.clone(), poll);

    // === PAYMENT SERVICE ===
    let payment_repo = HashMapRepository::new();
    let payment_worker =
        OutboxWorkerThread::spawn_routed(payment_repo.clone(), queue.clone(), poll);
    let payment_svc = Arc::new(sourced_rust::register_handlers!(
        Service::new(payment_repo.queued().aggregate::<Payment>()),
        handlers::payments::process,
    ));
    let payment_listen = microsvc::listen(payment_svc.clone(), "payments", queue.clone(), poll);

    // === START THE SAGA ===
    saga_svc
        .dispatch(
            "StartSaga",
            json!({
                "saga_id": "saga-001",
                "order_id": "order-001",
                "customer_id": "customer-001",
                "items": [{ "sku": "WIDGET-001", "quantity": 5, "price_cents": 1000 }],
                "total_cents": 5000
            }),
            Session::new(),
        )
        .unwrap();

    // === POLL FOR COMPLETION ===
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(saga) = saga_svc.repo().peek("saga-001").unwrap() {
            if saga.is_complete() {
                break;
            }
        }
        assert!(
            Instant::now() < deadline,
            "Saga should complete within 10 seconds"
        );
        thread::sleep(Duration::from_millis(50));
    }

    // === STOP TRANSPORTS AND WORKERS ===
    let saga_stats = saga_listen.stop();
    let order_stats = order_listen.stop();
    let _inventory_stats = inventory_listen.stop();
    let _payment_stats = payment_listen.stop();

    saga_worker.stop();
    order_worker.stop();
    inventory_worker.stop();
    payment_worker.stop();

    // === VERIFY FINAL STATE — typed repos return aggregates directly ===

    let saga = saga_svc.repo().peek("saga-001").unwrap().unwrap();
    assert_eq!(saga.status(), SagaStatus::Completed);
    assert!(saga.is_complete());

    let order = order_svc.repo().peek("order-001").unwrap().unwrap();
    assert_eq!(order.status(), OrderStatus::Completed);

    let inv = inventory_svc.repo().peek("WIDGET-001").unwrap().unwrap();
    assert_eq!(inv.available(), 95);
    assert_eq!(inv.reserved(), 5);

    let payment = payment_svc.repo().peek("pay-order-001").unwrap().unwrap();
    assert!(payment.is_successful());

    // Transport stats: saga handled 4 events, orders handled 2
    assert_eq!(saga_stats.handled, 4);
    assert_eq!(order_stats.handled, 2);
}
