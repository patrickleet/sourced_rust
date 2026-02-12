//! Typed Microservice Saga Tests
//!
//! Demonstrates `microsvc::Service::new(repo.queued().aggregate::<T>())` —
//! each service is typed to a specific aggregate, so handlers access
//! `ctx.repo().get()`, `ctx.repo().commit()`, etc. directly.
//!
//! Two tests:
//! 1. **Orchestrated** — test runner dispatches commands to each service
//! 2. **Distributed** — services communicate via bus with `microsvc::listen`

use std::sync::{mpsc::channel, Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::json;

use sourced_rust::microsvc::{self, HandlerError, Service, Session};
use sourced_rust::{
    AggregateBuilder, HashMapRepository, InMemoryQueue, OutboxCommitExt, OutboxMessage,
    OutboxWorkerThread, Queueable,
};

use super::order::{
    Inventory, Order, OrderFulfillmentSaga, OrderItem, OrderStatus, Payment, SagaStatus,
};

// ============================================================================
// Input types for command handlers
// ============================================================================

#[derive(Serialize, Deserialize)]
struct CreateOrderInput {
    order_id: String,
    customer_id: String,
    items: Vec<OrderItem>,
}

#[derive(Serialize, Deserialize)]
struct OrderIdInput {
    order_id: String,
}

#[derive(Serialize, Deserialize)]
struct InitInventoryInput {
    sku: String,
    stock: u32,
}

#[derive(Serialize, Deserialize)]
struct ReserveInventoryInput {
    order_id: String,
    sku: String,
    quantity: u32,
}

#[derive(Serialize, Deserialize)]
struct CommitReservationInput {
    order_id: String,
    sku: String,
}

#[derive(Serialize, Deserialize)]
struct ProcessPaymentInput {
    payment_id: String,
    order_id: String,
    amount_cents: u32,
}

#[derive(Serialize, Deserialize)]
struct StartSagaInput {
    saga_id: String,
    order_id: String,
    customer_id: String,
    items: Vec<OrderItem>,
    total_cents: u32,
}

#[derive(Serialize, Deserialize)]
struct SagaIdInput {
    saga_id: String,
}

// ============================================================================
// Test 1: Orchestrated — test runner dispatches to each service in sequence
// ============================================================================

/// Demonstrates typed microservices in a saga orchestration pattern.
///
/// Each service uses `Service::new(repo.queued().aggregate::<T>())` so that
/// handlers access the aggregate repo directly via `ctx.repo()` — no manual
/// `ctx.repo().clone().aggregate::<T>()` boilerplate.
///
/// ```text
///  ┌────────────────────────────────────────────────────────────────┐
///  │                    Test Runner (orchestrator)                   │
///  │                                                                │
///  │  dispatch ──→ [Saga Service]      typed to OrderFulfillmentSaga│
///  │  dispatch ──→ [Order Service]     typed to Order               │
///  │  dispatch ──→ [Inventory Service] typed to Inventory           │
///  │  dispatch ──→ [Payment Service]   typed to Payment             │
///  └────────────────────────────────────────────────────────────────┘
/// ```
#[test]
fn typed_microsvc_saga_orchestrated() {
    // === ORDER SERVICE (typed to Order) ===
    let order_svc = Service::new(HashMapRepository::new().queued().aggregate::<Order>())
        .command("order.create", |ctx| {
            let input = ctx.input::<CreateOrderInput>()?;
            let mut order = Order::new();
            order.create(input.order_id.clone(), input.customer_id, input.items);
            ctx.repo().commit(&mut order)?;
            Ok(json!({ "order_id": input.order_id }))
        })
        .command("order.complete", |ctx| {
            let input = ctx.input::<OrderIdInput>()?;
            let mut order = ctx
                .repo()
                .get(&input.order_id)?
                .ok_or_else(|| HandlerError::NotFound(input.order_id.clone()))?;
            order.mark_inventory_reserved();
            order.mark_payment_processed();
            order.complete();
            ctx.repo().commit(&mut order)?;
            Ok(json!({ "status": "completed" }))
        });

    // === INVENTORY SERVICE (typed to Inventory) ===
    let inventory_svc = Service::new(HashMapRepository::new().queued().aggregate::<Inventory>())
        .command("inventory.initialize", |ctx| {
            let input = ctx.input::<InitInventoryInput>()?;
            let mut inv = Inventory::new();
            inv.initialize(input.sku.clone(), input.stock);
            ctx.repo().commit(&mut inv)?;
            Ok(json!({ "sku": input.sku, "stock": input.stock }))
        })
        .command("inventory.reserve", |ctx| {
            let input = ctx.input::<ReserveInventoryInput>()?;
            let mut inv = ctx
                .repo()
                .get(&input.sku)?
                .ok_or_else(|| HandlerError::NotFound(input.sku.clone()))?;
            if !inv.can_reserve(input.quantity) {
                return Err(HandlerError::Rejected("insufficient stock".into()));
            }
            inv.reserve(input.order_id.clone(), input.quantity);
            ctx.repo().commit(&mut inv)?;
            Ok(json!({ "reserved": input.quantity }))
        })
        .command("inventory.commit_reservation", |ctx| {
            let input = ctx.input::<CommitReservationInput>()?;
            let mut inv = ctx
                .repo()
                .get(&input.sku)?
                .ok_or_else(|| HandlerError::NotFound(input.sku.clone()))?;
            inv.commit_reservation(input.order_id);
            ctx.repo().commit(&mut inv)?;
            Ok(json!({ "committed": true }))
        });

    // === PAYMENT SERVICE (typed to Payment) ===
    let payment_svc = Service::new(HashMapRepository::new().queued().aggregate::<Payment>())
        .command("payment.process", |ctx| {
            let input = ctx.input::<ProcessPaymentInput>()?;
            let mut payment = Payment::new();
            payment.initiate(
                input.payment_id.clone(),
                input.order_id.clone(),
                input.amount_cents,
            );
            payment.authorize("txn-001".to_string());
            payment.capture();
            ctx.repo().commit(&mut payment)?;
            Ok(json!({ "payment_id": input.payment_id, "status": "captured" }))
        });

    // === SAGA SERVICE (typed to OrderFulfillmentSaga) ===
    let saga_svc =
        Service::new(HashMapRepository::new().queued().aggregate::<OrderFulfillmentSaga>())
            .command("saga.start", |ctx| {
                let input = ctx.input::<StartSagaInput>()?;
                let mut saga = OrderFulfillmentSaga::new();
                saga.start(
                    input.saga_id.clone(),
                    input.order_id,
                    input.customer_id,
                    input.items,
                    input.total_cents,
                );
                ctx.repo().commit(&mut saga)?;
                Ok(json!({ "saga_id": input.saga_id, "status": "started" }))
            })
            .command("saga.inventory_reserved", |ctx| {
                let input = ctx.input::<SagaIdInput>()?;
                let mut saga = ctx
                    .repo()
                    .get(&input.saga_id)?
                    .ok_or_else(|| HandlerError::NotFound(input.saga_id.clone()))?;
                saga.inventory_reserved();
                ctx.repo().commit(&mut saga)?;
                Ok(json!({ "status": "inventory_reserved" }))
            })
            .command("saga.payment_succeeded", |ctx| {
                let input = ctx.input::<SagaIdInput>()?;
                let mut saga = ctx
                    .repo()
                    .get(&input.saga_id)?
                    .ok_or_else(|| HandlerError::NotFound(input.saga_id.clone()))?;
                saga.payment_succeeded();
                ctx.repo().commit(&mut saga)?;
                Ok(json!({ "status": "payment_succeeded" }))
            })
            .command("saga.complete", |ctx| {
                let input = ctx.input::<SagaIdInput>()?;
                let mut saga = ctx
                    .repo()
                    .get(&input.saga_id)?
                    .ok_or_else(|| HandlerError::NotFound(input.saga_id.clone()))?;
                saga.complete();
                ctx.repo().commit(&mut saga)?;
                Ok(json!({ "status": "completed" }))
            });

    let s = Session::new;

    // ========================================================================
    // Saga Flow
    // ========================================================================

    // 1. Initialize inventory
    inventory_svc
        .dispatch(
            "inventory.initialize",
            json!({ "sku": "WIDGET-001", "stock": 100 }),
            s(),
        )
        .unwrap();

    // 2. Start saga
    let result = saga_svc
        .dispatch(
            "saga.start",
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
    assert_eq!(result["status"], "started");

    // 3. Create order
    let result = order_svc
        .dispatch(
            "order.create",
            json!({
                "order_id": "order-001",
                "customer_id": "customer-001",
                "items": [{ "sku": "WIDGET-001", "quantity": 5, "price_cents": 1000 }]
            }),
            s(),
        )
        .unwrap();
    assert_eq!(result["order_id"], "order-001");

    // 4. Reserve inventory
    let result = inventory_svc
        .dispatch(
            "inventory.reserve",
            json!({ "order_id": "order-001", "sku": "WIDGET-001", "quantity": 5 }),
            s(),
        )
        .unwrap();
    assert_eq!(result["reserved"], 5);

    // 5. Advance saga: inventory reserved
    saga_svc
        .dispatch(
            "saga.inventory_reserved",
            json!({ "saga_id": "saga-001" }),
            s(),
        )
        .unwrap();

    // 6. Process payment
    let result = payment_svc
        .dispatch(
            "payment.process",
            json!({
                "payment_id": "pay-001",
                "order_id": "order-001",
                "amount_cents": 5000
            }),
            s(),
        )
        .unwrap();
    assert_eq!(result["status"], "captured");

    // 7. Advance saga: payment succeeded
    saga_svc
        .dispatch(
            "saga.payment_succeeded",
            json!({ "saga_id": "saga-001" }),
            s(),
        )
        .unwrap();

    // 8. Complete order
    order_svc
        .dispatch("order.complete", json!({ "order_id": "order-001" }), s())
        .unwrap();

    // 9. Commit inventory reservation
    inventory_svc
        .dispatch(
            "inventory.commit_reservation",
            json!({ "order_id": "order-001", "sku": "WIDGET-001" }),
            s(),
        )
        .unwrap();

    // 10. Complete saga
    saga_svc
        .dispatch("saga.complete", json!({ "saga_id": "saga-001" }), s())
        .unwrap();

    // ========================================================================
    // Verify final state — each service's typed repo returns aggregates directly
    // ========================================================================

    let saga: OrderFulfillmentSaga = saga_svc.repo().peek("saga-001").unwrap().unwrap();
    assert_eq!(saga.status(), SagaStatus::Completed);
    assert!(saga.is_complete());

    let order: Order = order_svc.repo().peek("order-001").unwrap().unwrap();
    assert_eq!(order.status(), OrderStatus::Completed);

    let inventory: Inventory = inventory_svc.repo().peek("WIDGET-001").unwrap().unwrap();
    assert_eq!(inventory.available(), 95); // 100 - 5
    assert_eq!(inventory.reserved(), 0);

    let payment: Payment = payment_svc.repo().peek("pay-001").unwrap().unwrap();
    assert!(payment.is_successful());
}

// ============================================================================
// Helper: JSON-serialized outbox message (microsvc dispatch expects JSON)
// ============================================================================

fn json_outbox_to<T: Serialize>(
    id: &str,
    event_type: &str,
    destination: &str,
    payload: &T,
) -> OutboxMessage {
    let bytes = serde_json::to_vec(payload).expect("JSON serialization should not fail");
    OutboxMessage::create_to(id, event_type, destination, bytes)
}

// ============================================================================
// Inter-service message types (for distributed test)
// ============================================================================

#[derive(Serialize, Deserialize)]
struct CreateOrderMsg {
    saga_id: String,
    order_id: String,
    customer_id: String,
    items: Vec<OrderItem>,
    total_cents: u32,
}

#[derive(Serialize, Deserialize)]
struct OrderCreatedMsg {
    saga_id: String,
    order_id: String,
}

#[derive(Serialize, Deserialize)]
struct ReserveInventoryMsg {
    saga_id: String,
    order_id: String,
    sku: String,
    quantity: u32,
}

#[derive(Serialize, Deserialize)]
struct InventoryReservedMsg {
    saga_id: String,
    order_id: String,
}

#[derive(Serialize, Deserialize)]
struct ProcessPaymentMsg {
    saga_id: String,
    order_id: String,
    amount_cents: u32,
}

#[derive(Serialize, Deserialize)]
struct PaymentSucceededMsg {
    saga_id: String,
    order_id: String,
}

#[derive(Serialize, Deserialize)]
struct CompleteOrderMsg {
    saga_id: String,
    order_id: String,
}

#[derive(Serialize, Deserialize)]
struct OrderCompletedMsg {
    saga_id: String,
    order_id: String,
}

// ============================================================================
// Test 2: Distributed — services communicate via bus transport
// ============================================================================

/// Distributed saga using typed microservices with bus transport.
///
/// Each service:
/// - Has its own `HashMapRepository` (simulating separate databases)
/// - Uses `OutboxWorkerThread::spawn_routed` for reliable message routing
/// - Uses `microsvc::listen` on a named queue for incoming commands
/// - Commits aggregate + outbox message atomically via `ctx.repo().outbox()`
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
///  │          │  │          │  │              │  │          │
///  │ typed:   │  │ typed:   │  │ typed:       │  │ typed:   │
///  │  Saga    │  │  Order   │  │  Inventory   │  │  Payment │
///  └──────────┘  └──────────┘  └──────────────┘  └──────────┘
/// ```
///
/// Flow:
/// 1. Saga starts → sends CreateOrder to "orders"
/// 2. Order creates → sends OrderCreated to "saga"
/// 3. Saga advances → sends ReserveInventory to "inventory"
/// 4. Inventory reserves → sends InventoryReserved to "saga"
/// 5. Saga advances → sends ProcessPayment to "payments"
/// 6. Payment captures → sends PaymentSucceeded to "saga"
/// 7. Saga advances → sends CompleteOrder to "orders"
/// 8. Order completes → sends OrderCompleted to "saga"
/// 9. Saga completes
#[test]
fn typed_microsvc_saga_distributed() {
    let queue = InMemoryQueue::new();
    let poll = Duration::from_millis(10);

    // Completion signal — Arc<Mutex<Sender>> is Send + Sync for handler closures
    let (done_tx, done_rx) = channel::<String>();
    let done_signal = Arc::new(Mutex::new(done_tx));

    // ========================================================================
    // SAGA SERVICE — typed to OrderFulfillmentSaga
    // ========================================================================
    let saga_repo = HashMapRepository::new();
    let saga_worker =
        OutboxWorkerThread::spawn_routed(saga_repo.clone(), queue.clone(), poll);
    let saga_service = Arc::new({
        let signal = done_signal.clone();
        Service::new(saga_repo.queued().aggregate::<OrderFulfillmentSaga>())
            // Receives OrderCreated from order service
            .command("OrderCreated", |ctx| {
                let input = ctx.input::<OrderCreatedMsg>()?;
                let mut saga = ctx
                    .repo()
                    .get(&input.saga_id)?
                    .ok_or_else(|| HandlerError::NotFound(input.saga_id.clone()))?;

                // Advance saga — now send ReserveInventory to inventory service
                // (saga knows the items from its own state)
                let sku = saga.items()[0].sku.clone();
                let quantity = saga.items()[0].quantity;

                let mut msg = json_outbox_to(
                    &format!("{}-reserve-inventory", input.saga_id),
                    "ReserveInventory",
                    "inventory",
                    &ReserveInventoryMsg {
                        saga_id: input.saga_id.clone(),
                        order_id: input.order_id,
                        sku,
                        quantity,
                    },
                );

                ctx.repo().outbox(&mut msg).commit(&mut saga)?;
                Ok(json!({ "next": "ReserveInventory" }))
            })
            // Receives InventoryReserved from inventory service
            .command("InventoryReserved", |ctx| {
                let input = ctx.input::<InventoryReservedMsg>()?;
                let mut saga = ctx
                    .repo()
                    .get(&input.saga_id)?
                    .ok_or_else(|| HandlerError::NotFound(input.saga_id.clone()))?;
                saga.inventory_reserved();

                // Send ProcessPayment to payment service
                let mut msg = json_outbox_to(
                    &format!("{}-process-payment", input.saga_id),
                    "ProcessPayment",
                    "payments",
                    &ProcessPaymentMsg {
                        saga_id: input.saga_id.clone(),
                        order_id: input.order_id,
                        amount_cents: saga.total_cents(),
                    },
                );

                ctx.repo().outbox(&mut msg).commit(&mut saga)?;
                Ok(json!({ "next": "ProcessPayment" }))
            })
            // Receives PaymentSucceeded from payment service
            .command("PaymentSucceeded", |ctx| {
                let input = ctx.input::<PaymentSucceededMsg>()?;
                let mut saga = ctx
                    .repo()
                    .get(&input.saga_id)?
                    .ok_or_else(|| HandlerError::NotFound(input.saga_id.clone()))?;
                saga.payment_succeeded();

                // Send CompleteOrder to order service
                let mut msg = json_outbox_to(
                    &format!("{}-complete-order", input.saga_id),
                    "CompleteOrder",
                    "orders",
                    &CompleteOrderMsg {
                        saga_id: input.saga_id.clone(),
                        order_id: input.order_id,
                    },
                );

                ctx.repo().outbox(&mut msg).commit(&mut saga)?;
                Ok(json!({ "next": "CompleteOrder" }))
            })
            // Receives OrderCompleted from order service — saga is done!
            .command("OrderCompleted", {
                move |ctx| {
                    let input = ctx.input::<OrderCompletedMsg>()?;
                    let mut saga = ctx
                        .repo()
                        .get(&input.saga_id)?
                        .ok_or_else(|| HandlerError::NotFound(input.saga_id.clone()))?;
                    saga.complete();
                    ctx.repo().commit(&mut saga)?;

                    // Signal the test that the saga is done
                    signal
                        .lock()
                        .unwrap()
                        .send(input.saga_id.clone())
                        .unwrap();
                    Ok(json!({ "status": "completed" }))
                }
            })
    });
    let saga_listen =
        microsvc::listen(saga_service.clone(), "saga", queue.clone(), poll);

    // ========================================================================
    // ORDER SERVICE — typed to Order
    // ========================================================================
    let order_repo = HashMapRepository::new();
    let order_worker =
        OutboxWorkerThread::spawn_routed(order_repo.clone(), queue.clone(), poll);
    let order_service = Arc::new(
        Service::new(order_repo.queued().aggregate::<Order>())
            // Receives CreateOrder from saga
            .command("CreateOrder", |ctx| {
                let input = ctx.input::<CreateOrderMsg>()?;
                let mut order = Order::new();
                order.create(
                    input.order_id.clone(),
                    input.customer_id,
                    input.items,
                );

                // Send OrderCreated back to saga
                let mut msg = json_outbox_to(
                    &format!("{}-order-created", input.order_id),
                    "OrderCreated",
                    "saga",
                    &OrderCreatedMsg {
                        saga_id: input.saga_id,
                        order_id: input.order_id,
                    },
                );

                ctx.repo().outbox(&mut msg).commit(&mut order)?;
                Ok(json!({ "created": true }))
            })
            // Receives CompleteOrder from saga
            .command("CompleteOrder", |ctx| {
                let input = ctx.input::<CompleteOrderMsg>()?;
                let mut order = ctx
                    .repo()
                    .get(&input.order_id)?
                    .ok_or_else(|| HandlerError::NotFound(input.order_id.clone()))?;
                order.mark_inventory_reserved();
                order.mark_payment_processed();
                order.complete();

                // Send OrderCompleted back to saga
                let mut msg = json_outbox_to(
                    &format!("{}-order-completed", input.order_id),
                    "OrderCompleted",
                    "saga",
                    &OrderCompletedMsg {
                        saga_id: input.saga_id,
                        order_id: input.order_id,
                    },
                );

                ctx.repo().outbox(&mut msg).commit(&mut order)?;
                Ok(json!({ "completed": true }))
            }),
    );
    let order_listen =
        microsvc::listen(order_service.clone(), "orders", queue.clone(), poll);

    // ========================================================================
    // INVENTORY SERVICE — typed to Inventory
    // ========================================================================
    let inventory_repo = HashMapRepository::new();
    let inventory_worker =
        OutboxWorkerThread::spawn_routed(inventory_repo.clone(), queue.clone(), poll);

    // Initialize inventory before starting the service
    {
        let tmp = inventory_repo.clone().aggregate::<Inventory>();
        let mut inv = Inventory::new();
        inv.initialize("WIDGET-001".to_string(), 100);
        tmp.commit(&mut inv).unwrap();
    }

    let inventory_service = Arc::new(
        Service::new(inventory_repo.queued().aggregate::<Inventory>())
            .command("ReserveInventory", |ctx| {
                let input = ctx.input::<ReserveInventoryMsg>()?;
                let mut inv = ctx
                    .repo()
                    .get(&input.sku)?
                    .ok_or_else(|| HandlerError::NotFound(input.sku.clone()))?;

                if !inv.can_reserve(input.quantity) {
                    return Err(HandlerError::Rejected("insufficient stock".into()));
                }
                inv.reserve(input.order_id.clone(), input.quantity);

                // Send InventoryReserved back to saga
                let mut msg = json_outbox_to(
                    &format!("{}-inventory-reserved", input.order_id),
                    "InventoryReserved",
                    "saga",
                    &InventoryReservedMsg {
                        saga_id: input.saga_id,
                        order_id: input.order_id,
                    },
                );

                ctx.repo().outbox(&mut msg).commit(&mut inv)?;
                Ok(json!({ "reserved": input.quantity }))
            }),
    );
    let inventory_listen =
        microsvc::listen(inventory_service.clone(), "inventory", queue.clone(), poll);

    // ========================================================================
    // PAYMENT SERVICE — typed to Payment
    // ========================================================================
    let payment_repo = HashMapRepository::new();
    let payment_worker =
        OutboxWorkerThread::spawn_routed(payment_repo.clone(), queue.clone(), poll);
    let payment_service = Arc::new(
        Service::new(payment_repo.queued().aggregate::<Payment>())
            .command("ProcessPayment", |ctx| {
                let input = ctx.input::<ProcessPaymentMsg>()?;
                let payment_id = format!("pay-{}", input.order_id);
                let mut payment = Payment::new();
                payment.initiate(payment_id.clone(), input.order_id.clone(), input.amount_cents);
                payment.authorize("txn-distributed-001".to_string());
                payment.capture();

                // Send PaymentSucceeded back to saga
                let mut msg = json_outbox_to(
                    &format!("{}-payment-succeeded", input.order_id),
                    "PaymentSucceeded",
                    "saga",
                    &PaymentSucceededMsg {
                        saga_id: input.saga_id,
                        order_id: input.order_id,
                    },
                );

                ctx.repo().outbox(&mut msg).commit(&mut payment)?;
                Ok(json!({ "payment_id": payment_id }))
            }),
    );
    let payment_listen =
        microsvc::listen(payment_service.clone(), "payments", queue.clone(), poll);

    // ========================================================================
    // START THE SAGA — direct dispatch kicks everything off
    // ========================================================================

    // Create the saga and send the first command to the orders queue
    {
        let saga_id = "saga-001";
        let order_id = "order-001";
        let items = vec![OrderItem {
            sku: "WIDGET-001".to_string(),
            quantity: 5,
            price_cents: 1000,
        }];

        // Create saga aggregate
        let mut saga = OrderFulfillmentSaga::new();
        saga.start(
            saga_id.to_string(),
            order_id.to_string(),
            "customer-001".to_string(),
            items.clone(),
            5000,
        );

        // Create outbox message routed to "orders" queue
        let mut msg = json_outbox_to(
            &format!("{}-create-order", saga_id),
            "CreateOrder",
            "orders",
            &CreateOrderMsg {
                saga_id: saga_id.to_string(),
                order_id: order_id.to_string(),
                customer_id: "customer-001".to_string(),
                items,
                total_cents: 5000,
            },
        );

        // Commit saga + outbox atomically through the typed service repo
        saga_service
            .repo()
            .outbox(&mut msg)
            .commit(&mut saga)
            .unwrap();
    }

    // ========================================================================
    // WAIT FOR COMPLETION
    // ========================================================================

    let completed_saga = done_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("Saga should complete within 10 seconds");

    assert_eq!(completed_saga, "saga-001");

    // ========================================================================
    // STOP TRANSPORTS AND WORKERS
    // ========================================================================

    let saga_stats = saga_listen.stop();
    let order_stats = order_listen.stop();
    let _inventory_stats = inventory_listen.stop();
    let _payment_stats = payment_listen.stop();

    saga_worker.stop();
    order_worker.stop();
    inventory_worker.stop();
    payment_worker.stop();

    // ========================================================================
    // VERIFY FINAL STATE — typed repos return aggregates directly
    // ========================================================================

    // Saga completed
    let saga: OrderFulfillmentSaga = saga_service.repo().peek("saga-001").unwrap().unwrap();
    assert_eq!(saga.status(), SagaStatus::Completed);
    assert!(saga.is_complete());

    // Order completed
    let order: Order = order_service.repo().peek("order-001").unwrap().unwrap();
    assert_eq!(order.status(), OrderStatus::Completed);

    // Inventory reserved (5 units from 100)
    let inventory: Inventory = inventory_service
        .repo()
        .peek("WIDGET-001")
        .unwrap()
        .unwrap();
    assert_eq!(inventory.available(), 95);
    assert_eq!(inventory.reserved(), 5);

    // Payment captured
    let payment: Payment = payment_service
        .repo()
        .peek("pay-order-001")
        .unwrap()
        .unwrap();
    assert!(payment.is_successful());

    // Transport stats: saga handled 4 events, order handled 2
    assert_eq!(saga_stats.handled, 4);
    assert_eq!(order_stats.handled, 2);

    println!("Distributed saga completed successfully!");
    println!(
        "Event flow: CreateOrder → OrderCreated → ReserveInventory → \
         InventoryReserved → ProcessPayment → PaymentSucceeded → \
         CompleteOrder → OrderCompleted → SagaCompleted"
    );
}
