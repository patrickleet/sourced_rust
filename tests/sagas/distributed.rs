//! Distributed Saga with Outbox Pattern
//!
//! Demonstrates a distributed system where each service runs in its own thread:
//! - Saga orchestrator coordinates the overall flow
//! - Each service has its own repository and outbox worker
//! - Services communicate ONLY via the shared queue
//! - No direct calls between services
//!
//! ```text
//!  ┌─────────────────────────────────────────────────────────────┐
//!  │                   Shared Queue (InMemoryQueue)              │
//!  │              thread-safe via Arc<RwLock<...>>               │
//!  └─────────────────────────────────────────────────────────────┘
//!        ↑↓            ↑↓            ↑↓            ↑↓
//!  ┌───────────┐ ┌───────────┐ ┌───────────┐ ┌───────────┐
//!  │   Saga    │ │  Order    │ │ Inventory │ │  Payment  │
//!  │  Thread   │ │  Thread   │ │  Thread   │ │  Thread   │
//!  │           │ │           │ │           │ │           │
//!  │ orchestr. │ │ repo +    │ │ repo +    │ │ repo +    │
//!  │ + repo    │ │ worker    │ │ worker    │ │ worker    │
//!  └───────────┘ └───────────┘ └───────────┘ └───────────┘
//! ```

use super::order::{
    Inventory, InventoryReservedPayload, Order, OrderCreatedPayload,
    OrderFulfillmentCompletedPayload, OrderFulfillmentSaga, OrderFulfillmentStartedPayload,
    OrderItem, Payment, PaymentSucceededPayload,
};
use sourced_rust::{
    bus::Bus, AggregateBuilder, CommitBuilderExt, HashMapRepository, InMemoryQueue,
    OutboxCommitExt, OutboxMessage, OutboxWorkerThread, Queueable,
};
use std::sync::mpsc::channel;
use std::thread;
use std::time::Duration;

#[test]
fn distributed_saga_with_threads() {
    // Shared queue - all services publish to and subscribe from this
    let queue = InMemoryQueue::new();

    // Channel to signal saga completion
    let (complete_tx, complete_rx) = channel::<String>();

    // =========================================================================
    // SAGA ORCHESTRATOR THREAD
    // =========================================================================
    let order_fulfillment_saga_queue = queue.clone();
    let saga_complete_tx = complete_tx.clone();
    let order_fulfillment_saga_thread = thread::spawn(move || {
        let repo = HashMapRepository::new();
        let worker = OutboxWorkerThread::spawn(
            repo.clone(),
            order_fulfillment_saga_queue.clone(),
            Duration::from_millis(10),
        );
        let order_fulfillment_saga_repo = repo.queued().aggregate::<OrderFulfillmentSaga>();

        // Create a bus for this service
        let bus = Bus::from_queue(order_fulfillment_saga_queue);

        // === Start the saga ===
        let saga_id = "saga-001".to_string();
        let order_id = "order-001".to_string();
        let items = vec![OrderItem {
            sku: "WIDGET-001".to_string(),
            quantity: 5,
            price_cents: 1000,
        }];

        let mut order_fulfillment_saga = OrderFulfillmentSaga::new();
        order_fulfillment_saga.start(
            saga_id.clone(),
            order_id.clone(),
            "customer-001".to_string(),
            items.clone(),
            5000,
        );

        let mut outbox = OutboxMessage::encode(
            &format!("{}:started", saga_id),
            "SagaStarted",
            &OrderFulfillmentStartedPayload {
                saga_id: saga_id.clone(),
                order_id: order_id.clone(),
                customer_id: "customer-001".to_string(),
                items,
                total_cents: 5000,
            },
        )
        .unwrap();
        order_fulfillment_saga_repo
            .outbox(&mut outbox)
            .commit(&mut order_fulfillment_saga)
            .unwrap();

        println!(
            "[Saga Orchestrator] Started saga {}, waiting for events...",
            saga_id
        );

        // === Subscribe to events that advance saga state ===
        let events = bus.subscribe(&[
            "OrderCreated",
            "InventoryReserved",
            "PaymentSucceeded",
            "OrderCompleted",
        ]);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);

        while std::time::Instant::now() < deadline {
            if let Ok(Some(event)) = events.recv(100) {
                match event.event_type.as_str() {
                    "OrderCreated" => {
                        let data: OrderCreatedPayload = event.decode().unwrap();
                        if data.order_id == order_id {
                            println!("[Saga Orchestrator] Order created, waiting for inventory...");
                        }
                    }
                    "InventoryReserved" => {
                        let data: InventoryReservedPayload = event.decode().unwrap();
                        if data.order_id == order_id {
                            println!("[Saga Orchestrator] Inventory reserved, advancing saga...");
                            let mut order_fulfillment_saga =
                                order_fulfillment_saga_repo.get(&saga_id).unwrap().unwrap();
                            order_fulfillment_saga.inventory_reserved();
                            order_fulfillment_saga_repo
                                .commit(&mut order_fulfillment_saga)
                                .unwrap();
                        }
                    }
                    "PaymentSucceeded" => {
                        let data: PaymentSucceededPayload = event.decode().unwrap();
                        if data.order_id == order_id {
                            println!("[Saga Orchestrator] Payment succeeded, advancing saga...");
                            let mut order_fulfillment_saga =
                                order_fulfillment_saga_repo.get(&saga_id).unwrap().unwrap();
                            order_fulfillment_saga.payment_succeeded();
                            order_fulfillment_saga_repo
                                .commit(&mut order_fulfillment_saga)
                                .unwrap();
                        }
                    }
                    "OrderCompleted" => {
                        let data: PaymentSucceededPayload = event.decode().unwrap();
                        if data.order_id == order_id {
                            println!("[Saga Orchestrator] Order completed, completing saga...");
                            let mut order_fulfillment_saga =
                                order_fulfillment_saga_repo.get(&saga_id).unwrap().unwrap();
                            order_fulfillment_saga.complete();

                            let mut outbox = OutboxMessage::encode(
                                &format!("{}:completed", saga_id),
                                "SagaCompleted",
                                &OrderFulfillmentCompletedPayload {
                                    saga_id: saga_id.clone(),
                                    order_id: order_id.clone(),
                                },
                            )
                            .unwrap();
                            order_fulfillment_saga_repo
                                .outbox(&mut outbox)
                                .commit(&mut order_fulfillment_saga)
                                .unwrap();

                            // Wait for outbox worker to publish
                            thread::sleep(Duration::from_millis(50));

                            println!("[Saga Orchestrator] Saga completed!");
                            saga_complete_tx.send(saga_id.clone()).unwrap();
                            break;
                        }
                    }
                    _ => unreachable!("Subscribed events are filtered"),
                }
            }
        }

        let final_order_fulfillment_saga =
            order_fulfillment_saga_repo.peek(&saga_id).unwrap().unwrap();
        println!(
            "[Saga Orchestrator] Final saga status: {:?}",
            final_order_fulfillment_saga.status()
        );

        worker.stop();
    });

    // =========================================================================
    // ORDER SERVICE THREAD
    // =========================================================================
    let order_queue = queue.clone();
    let order_thread = thread::spawn(move || {
        let repo = HashMapRepository::new();
        let worker =
            OutboxWorkerThread::spawn(repo.clone(), order_queue.clone(), Duration::from_millis(10));
        let order_repo = repo.queued().aggregate::<Order>();

        // Create a bus for this service
        let bus = Bus::from_queue(order_queue);

        println!("[Order Service] Waiting for SagaStarted...");

        // Subscribe only to events this service cares about
        let events = bus.subscribe(&["SagaStarted", "PaymentSucceeded"]);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut order_id: Option<String> = None;

        while std::time::Instant::now() < deadline {
            if let Ok(Some(event)) = events.recv(100) {
                match event.event_type.as_str() {
                    "SagaStarted" => {
                        let data: OrderFulfillmentStartedPayload = event.decode().unwrap();
                        println!("[Order Service] Received SagaStarted, creating order...");

                        let mut order = Order::new();
                        order.create(
                            data.order_id.clone(),
                            data.customer_id.clone(),
                            data.items.clone(),
                        );

                        let mut outbox = OutboxMessage::encode(
                            &format!("{}:created", data.order_id),
                            "OrderCreated",
                            &OrderCreatedPayload {
                                order_id: data.order_id.clone(),
                                customer_id: data.customer_id,
                                items: data.items,
                                total_cents: data.total_cents,
                            },
                        )
                        .unwrap();
                        order_repo.outbox(&mut outbox).commit(&mut order).unwrap();

                        println!("[Order Service] Created order {}", data.order_id);
                        order_id = Some(data.order_id);

                        thread::sleep(Duration::from_millis(50));
                    }
                    "PaymentSucceeded" => {
                        let data: PaymentSucceededPayload = event.decode().unwrap();
                        if Some(&data.order_id) == order_id.as_ref() {
                            println!(
                                "[Order Service] Received PaymentSucceeded, completing order..."
                            );

                            let mut order = order_repo.get(&data.order_id).unwrap().unwrap();
                            order.mark_inventory_reserved();
                            order.mark_payment_processed();
                            order.complete();

                            let mut outbox = OutboxMessage::encode(
                                &format!("{}:completed", data.order_id),
                                "OrderCompleted",
                                &data,
                            )
                            .unwrap();
                            order_repo.outbox(&mut outbox).commit(&mut order).unwrap();

                            println!("[Order Service] Order completed!");
                            thread::sleep(Duration::from_millis(50));
                            break;
                        }
                    }
                    _ => unreachable!("Subscribed events are filtered"),
                }
            }
        }

        worker.stop();
    });

    // =========================================================================
    // INVENTORY SERVICE THREAD
    // =========================================================================
    let inventory_queue = queue.clone();
    let inventory_thread = thread::spawn(move || {
        let repo = HashMapRepository::new();
        let worker = OutboxWorkerThread::spawn(
            repo.clone(),
            inventory_queue.clone(),
            Duration::from_millis(10),
        );
        let inventory_repo = repo.queued().aggregate::<Inventory>();

        // Create a bus for this service
        let bus = Bus::from_queue(inventory_queue);

        // Initialize inventory first
        let mut inv = Inventory::new();
        inv.initialize("WIDGET-001".to_string(), 100);
        inventory_repo.commit(&mut inv).unwrap();

        println!(
            "[Inventory Service] Initialized with 100 WIDGET-001, waiting for OrderCreated..."
        );

        // Subscribe only to OrderCreated events
        let events = bus.subscribe(&["OrderCreated"]);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);

        while std::time::Instant::now() < deadline {
            if let Ok(Some(event)) = events.recv(100) {
                let data: OrderCreatedPayload = event.decode().unwrap();
                println!("[Inventory Service] Received OrderCreated, reserving inventory...");

                let item = &data.items[0];
                let mut inv = inventory_repo.get(&item.sku).unwrap().unwrap();

                if inv.can_reserve(item.quantity) {
                    inv.reserve(data.order_id.clone(), item.quantity);

                    let mut outbox = OutboxMessage::encode(
                        &format!("{}:reserved", data.order_id),
                        "InventoryReserved",
                        &InventoryReservedPayload {
                            order_id: data.order_id.clone(),
                            sku: item.sku.clone(),
                            quantity: item.quantity,
                        },
                    )
                    .unwrap();
                    inventory_repo.outbox(&mut outbox).commit(&mut inv).unwrap();

                    println!("[Inventory Service] Reserved {} units", item.quantity);
                    thread::sleep(Duration::from_millis(50));
                }
                break;
            }
        }

        worker.stop();
    });

    // =========================================================================
    // PAYMENT SERVICE THREAD
    // =========================================================================
    let payment_queue = queue.clone();
    let payment_thread = thread::spawn(move || {
        let repo = HashMapRepository::new();
        let worker = OutboxWorkerThread::spawn(
            repo.clone(),
            payment_queue.clone(),
            Duration::from_millis(10),
        );
        let payment_repo = repo.queued().aggregate::<Payment>();

        // Create a bus for this service
        let bus = Bus::from_queue(payment_queue);

        println!("[Payment Service] Waiting for InventoryReserved...");

        // Subscribe only to InventoryReserved events
        let events = bus.subscribe(&["InventoryReserved"]);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);

        while std::time::Instant::now() < deadline {
            if let Ok(Some(event)) = events.recv(100) {
                let data: InventoryReservedPayload = event.decode().unwrap();
                println!("[Payment Service] Received InventoryReserved, processing payment...");

                let mut payment = Payment::new();
                let payment_id = format!("pay-{}", data.order_id);
                payment.initiate(payment_id.clone(), data.order_id.clone(), 5000);
                payment.authorize("txn-123".to_string());
                payment.capture();

                let mut outbox = OutboxMessage::encode(
                    &format!("{}:paid", data.order_id),
                    "PaymentSucceeded",
                    &PaymentSucceededPayload {
                        order_id: data.order_id.clone(),
                        payment_id,
                    },
                )
                .unwrap();
                payment_repo
                    .outbox(&mut outbox)
                    .commit(&mut payment)
                    .unwrap();

                println!("[Payment Service] Payment succeeded!");
                thread::sleep(Duration::from_millis(50));
                break;
            }
        }

        worker.stop();
    });

    // =========================================================================
    // WAIT FOR SAGA COMPLETION
    // =========================================================================

    let completed_saga = complete_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("Saga should complete within 10 seconds");

    assert_eq!(completed_saga, "saga-001");

    // Join all threads
    order_fulfillment_saga_thread
        .join()
        .expect("Saga thread panicked");
    order_thread.join().expect("Order thread panicked");
    inventory_thread.join().expect("Inventory thread panicked");
    payment_thread.join().expect("Payment thread panicked");

    // Verify the event flow
    let event_types = queue.event_types();
    println!("\nEvent flow: {:?}", event_types);

    assert!(event_types.contains(&"SagaStarted".to_string()));
    assert!(event_types.contains(&"OrderCreated".to_string()));
    assert!(event_types.contains(&"InventoryReserved".to_string()));
    assert!(event_types.contains(&"PaymentSucceeded".to_string()));
    assert!(event_types.contains(&"OrderCompleted".to_string()));
    assert!(event_types.contains(&"SagaCompleted".to_string()));
}

/// Distributed saga using point-to-point messaging (send/listen).
///
/// Same flow as `distributed_saga_with_threads`, but uses named queues
/// instead of fan-out pub/sub. Each service has its own queue:
///
/// ```text
///  ┌─────────────────────────────────────────────────────────────┐
///  │              Shared Queue (InMemoryQueue)                    │
///  │         named queues via send/listen (point-to-point)        │
///  └─────────────────────────────────────────────────────────────┘
///   "saga"          "orders"       "inventory"      "payments"
///    ↑↓               ↑↓              ↑↓               ↑↓
///  ┌───────────┐ ┌───────────┐ ┌───────────┐ ┌───────────┐
///  │   Saga    │ │  Order    │ │ Inventory │ │  Payment  │
///  │  Thread   │ │  Thread   │ │  Thread   │ │  Thread   │
///  └───────────┘ └───────────┘ └───────────┘ └───────────┘
/// ```
///
/// The outbox worker uses `spawn_routed` which checks `msg.destination`
/// and routes via `send(queue, event)` instead of `publish(event)`.
#[test]
fn distributed_saga_with_send_listen() {
    // Shared queue - all services send to named queues within this
    let queue = InMemoryQueue::new();

    // Channel to signal saga completion
    let (complete_tx, complete_rx) = channel::<String>();

    // =========================================================================
    // SAGA ORCHESTRATOR THREAD
    // =========================================================================
    let order_fulfillment_saga_queue = queue.clone();
    let saga_complete_tx = complete_tx.clone();
    let order_fulfillment_saga_thread = thread::spawn(move || {
        let repo = HashMapRepository::new();
        // spawn_routed: checks msg.destination → send() if set, publish() if not
        let worker = OutboxWorkerThread::spawn_routed(
            repo.clone(),
            order_fulfillment_saga_queue.clone(),
            Duration::from_millis(10),
        );
        let order_fulfillment_saga_repo = repo.queued().aggregate::<OrderFulfillmentSaga>();

        let bus = Bus::from_queue(order_fulfillment_saga_queue);

        // === Start the saga ===
        let saga_id = "saga-001".to_string();
        let order_id = "order-001".to_string();
        let items = vec![OrderItem {
            sku: "WIDGET-001".to_string(),
            quantity: 5,
            price_cents: 1000,
        }];

        let mut order_fulfillment_saga = OrderFulfillmentSaga::new();
        order_fulfillment_saga.start(
            saga_id.clone(),
            order_id.clone(),
            "customer-001".to_string(),
            items.clone(),
            5000,
        );

        // Send to the "orders" queue (point-to-point)
        let mut outbox = OutboxMessage::encode_to(
            &format!("{}:started", saga_id),
            "SagaStarted",
            "orders", // destination queue
            &OrderFulfillmentStartedPayload {
                saga_id: saga_id.clone(),
                order_id: order_id.clone(),
                customer_id: "customer-001".to_string(),
                items,
                total_cents: 5000,
            },
        )
        .unwrap();
        order_fulfillment_saga_repo
            .outbox(&mut outbox)
            .commit(&mut order_fulfillment_saga)
            .unwrap();

        println!(
            "[Saga/SendListen] Started saga {}, listening on 'saga' queue...",
            saga_id
        );

        // === Listen on the "saga" queue for events that advance saga state ===
        let deadline = std::time::Instant::now() + Duration::from_secs(10);

        while std::time::Instant::now() < deadline {
            if let Ok(Some(event)) = bus.listen("saga", 100) {
                match event.event_type.as_str() {
                    "OrderCreated" => {
                        let data: OrderCreatedPayload = event.decode().unwrap();
                        if data.order_id == order_id {
                            println!("[Saga/SendListen] Order created, waiting for inventory...");
                        }
                    }
                    "InventoryReserved" => {
                        let data: InventoryReservedPayload = event.decode().unwrap();
                        if data.order_id == order_id {
                            println!("[Saga/SendListen] Inventory reserved, advancing saga...");
                            let mut order_fulfillment_saga =
                                order_fulfillment_saga_repo.get(&saga_id).unwrap().unwrap();
                            order_fulfillment_saga.inventory_reserved();
                            order_fulfillment_saga_repo
                                .commit(&mut order_fulfillment_saga)
                                .unwrap();
                        }
                    }
                    "PaymentSucceeded" => {
                        let data: PaymentSucceededPayload = event.decode().unwrap();
                        if data.order_id == order_id {
                            println!("[Saga/SendListen] Payment succeeded, advancing saga...");
                            let mut order_fulfillment_saga =
                                order_fulfillment_saga_repo.get(&saga_id).unwrap().unwrap();
                            order_fulfillment_saga.payment_succeeded();
                            order_fulfillment_saga_repo
                                .commit(&mut order_fulfillment_saga)
                                .unwrap();
                        }
                    }
                    "OrderCompleted" => {
                        let data: PaymentSucceededPayload = event.decode().unwrap();
                        if data.order_id == order_id {
                            println!("[Saga/SendListen] Order completed, completing saga...");
                            let mut order_fulfillment_saga =
                                order_fulfillment_saga_repo.get(&saga_id).unwrap().unwrap();
                            order_fulfillment_saga.complete();

                            // SagaCompleted has no specific destination, but we can
                            // still route it to a queue (or use publish for fan-out)
                            let mut outbox = OutboxMessage::encode_to(
                                &format!("{}:completed", saga_id),
                                "SagaCompleted",
                                "saga-completed", // destination queue
                                &OrderFulfillmentCompletedPayload {
                                    saga_id: saga_id.clone(),
                                    order_id: order_id.clone(),
                                },
                            )
                            .unwrap();
                            order_fulfillment_saga_repo
                                .outbox(&mut outbox)
                                .commit(&mut order_fulfillment_saga)
                                .unwrap();

                            thread::sleep(Duration::from_millis(50));

                            println!("[Saga/SendListen] Saga completed!");
                            saga_complete_tx.send(saga_id.clone()).unwrap();
                            break;
                        }
                    }
                    other => {
                        println!("[Saga/SendListen] Unexpected event: {}", other);
                    }
                }
            }
        }

        let final_order_fulfillment_saga =
            order_fulfillment_saga_repo.peek(&saga_id).unwrap().unwrap();
        println!(
            "[Saga/SendListen] Final saga status: {:?}",
            final_order_fulfillment_saga.status()
        );

        worker.stop();
    });

    // =========================================================================
    // ORDER SERVICE THREAD
    // =========================================================================
    let order_queue = queue.clone();
    let order_thread = thread::spawn(move || {
        let repo = HashMapRepository::new();
        let worker = OutboxWorkerThread::spawn_routed(
            repo.clone(),
            order_queue.clone(),
            Duration::from_millis(10),
        );
        let order_repo = repo.clone().queued().aggregate::<Order>();

        let bus = Bus::from_queue(order_queue);

        println!("[Order/SendListen] Listening on 'orders' queue...");

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut order_id: Option<String> = None;

        while std::time::Instant::now() < deadline {
            if let Ok(Some(event)) = bus.listen("orders", 100) {
                match event.event_type.as_str() {
                    "SagaStarted" => {
                        let data: OrderFulfillmentStartedPayload = event.decode().unwrap();
                        println!("[Order/SendListen] Received SagaStarted, creating order...");

                        let mut order = Order::new();
                        order.create(
                            data.order_id.clone(),
                            data.customer_id.clone(),
                            data.items.clone(),
                        );

                        // Send OrderCreated to both "saga" and "inventory" queues
                        let payload = OrderCreatedPayload {
                            order_id: data.order_id.clone(),
                            customer_id: data.customer_id,
                            items: data.items,
                            total_cents: data.total_cents,
                        };

                        let outbox_saga = OutboxMessage::encode_to(
                            &format!("{}:created:saga", data.order_id),
                            "OrderCreated",
                            "saga",
                            &payload,
                        )
                        .unwrap();

                        let outbox_inventory = OutboxMessage::encode_to(
                            &format!("{}:created:inventory", data.order_id),
                            "OrderCreated",
                            "inventory",
                            &payload,
                        )
                        .unwrap();

                        repo.outbox(outbox_saga)
                            .outbox(outbox_inventory)
                            .commit(&mut order)
                            .unwrap();

                        println!("[Order/SendListen] Created order {}", data.order_id);
                        order_id = Some(data.order_id);

                        thread::sleep(Duration::from_millis(50));
                    }
                    "PaymentSucceeded" => {
                        let data: PaymentSucceededPayload = event.decode().unwrap();
                        if Some(&data.order_id) == order_id.as_ref() {
                            println!(
                                "[Order/SendListen] Received PaymentSucceeded, completing order..."
                            );

                            let mut order = order_repo.get(&data.order_id).unwrap().unwrap();
                            order.mark_inventory_reserved();
                            order.mark_payment_processed();
                            order.complete();

                            let mut outbox = OutboxMessage::encode_to(
                                &format!("{}:completed", data.order_id),
                                "OrderCompleted",
                                "saga",
                                &data,
                            )
                            .unwrap();
                            order_repo.outbox(&mut outbox).commit(&mut order).unwrap();

                            println!("[Order/SendListen] Order completed!");
                            thread::sleep(Duration::from_millis(50));
                            break;
                        }
                    }
                    other => {
                        println!("[Order/SendListen] Unexpected event: {}", other);
                    }
                }
            }
        }

        worker.stop();
    });

    // =========================================================================
    // INVENTORY SERVICE THREAD
    // =========================================================================
    let inventory_queue = queue.clone();
    let inventory_thread = thread::spawn(move || {
        let repo = HashMapRepository::new();
        let worker = OutboxWorkerThread::spawn_routed(
            repo.clone(),
            inventory_queue.clone(),
            Duration::from_millis(10),
        );
        let inventory_repo = repo.clone().queued().aggregate::<Inventory>();

        let bus = Bus::from_queue(inventory_queue);

        // Initialize inventory
        let mut inv = Inventory::new();
        inv.initialize("WIDGET-001".to_string(), 100);
        inventory_repo.commit(&mut inv).unwrap();

        println!(
            "[Inventory/SendListen] Initialized with 100 WIDGET-001, listening on 'inventory' queue..."
        );

        let deadline = std::time::Instant::now() + Duration::from_secs(5);

        while std::time::Instant::now() < deadline {
            if let Ok(Some(event)) = bus.listen("inventory", 100) {
                let data: OrderCreatedPayload = event.decode().unwrap();
                println!("[Inventory/SendListen] Received OrderCreated, reserving inventory...");

                let item = &data.items[0];
                let mut inv = inventory_repo.get(&item.sku).unwrap().unwrap();

                if inv.can_reserve(item.quantity) {
                    inv.reserve(data.order_id.clone(), item.quantity);

                    // Send InventoryReserved to both "saga" and "payments" queues
                    let payload = InventoryReservedPayload {
                        order_id: data.order_id.clone(),
                        sku: item.sku.clone(),
                        quantity: item.quantity,
                    };

                    let outbox_saga = OutboxMessage::encode_to(
                        &format!("{}:reserved:saga", data.order_id),
                        "InventoryReserved",
                        "saga",
                        &payload,
                    )
                    .unwrap();

                    let outbox_payments = OutboxMessage::encode_to(
                        &format!("{}:reserved:payments", data.order_id),
                        "InventoryReserved",
                        "payments",
                        &payload,
                    )
                    .unwrap();

                    repo.outbox(outbox_saga)
                        .outbox(outbox_payments)
                        .commit(&mut inv)
                        .unwrap();

                    println!("[Inventory/SendListen] Reserved {} units", item.quantity);
                    thread::sleep(Duration::from_millis(50));
                }
                break;
            }
        }

        worker.stop();
    });

    // =========================================================================
    // PAYMENT SERVICE THREAD
    // =========================================================================
    let payment_queue = queue.clone();
    let payment_thread = thread::spawn(move || {
        let repo = HashMapRepository::new();
        let worker = OutboxWorkerThread::spawn_routed(
            repo.clone(),
            payment_queue.clone(),
            Duration::from_millis(10),
        );
        let _payment_repo = repo.clone().queued().aggregate::<Payment>();

        let bus = Bus::from_queue(payment_queue);

        println!("[Payment/SendListen] Listening on 'payments' queue...");

        let deadline = std::time::Instant::now() + Duration::from_secs(5);

        while std::time::Instant::now() < deadline {
            if let Ok(Some(event)) = bus.listen("payments", 100) {
                let data: InventoryReservedPayload = event.decode().unwrap();
                println!("[Payment/SendListen] Received InventoryReserved, processing payment...");

                let mut payment = Payment::new();
                let payment_id = format!("pay-{}", data.order_id);
                payment.initiate(payment_id.clone(), data.order_id.clone(), 5000);
                payment.authorize("txn-123".to_string());
                payment.capture();

                // Send PaymentSucceeded to both "saga" and "orders" queues
                let payload = PaymentSucceededPayload {
                    order_id: data.order_id.clone(),
                    payment_id,
                };

                let outbox_saga = OutboxMessage::encode_to(
                    &format!("{}:paid:saga", data.order_id),
                    "PaymentSucceeded",
                    "saga",
                    &payload,
                )
                .unwrap();

                let outbox_orders = OutboxMessage::encode_to(
                    &format!("{}:paid:orders", data.order_id),
                    "PaymentSucceeded",
                    "orders",
                    &payload,
                )
                .unwrap();

                repo.outbox(outbox_saga)
                    .outbox(outbox_orders)
                    .commit(&mut payment)
                    .unwrap();

                println!("[Payment/SendListen] Payment succeeded!");
                thread::sleep(Duration::from_millis(50));
                break;
            }
        }

        worker.stop();
    });

    // =========================================================================
    // WAIT FOR SAGA COMPLETION
    // =========================================================================

    let completed_saga = complete_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("Saga should complete within 10 seconds");

    assert_eq!(completed_saga, "saga-001");

    // Join all threads
    order_fulfillment_saga_thread
        .join()
        .expect("Saga thread panicked");
    order_thread.join().expect("Order thread panicked");
    inventory_thread.join().expect("Inventory thread panicked");
    payment_thread.join().expect("Payment thread panicked");

    // With send/listen, events go to named queues, not the fan-out log.
    // Verify that the fan-out log is empty (no publish calls were made).
    let event_types = queue.event_types();
    assert!(
        event_types.is_empty(),
        "Fan-out log should be empty when using send/listen, got: {:?}",
        event_types
    );
}
