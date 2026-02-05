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

use sourced_rust::{
    bus::Bus, AggregateBuilder, HashMapRepository, InMemoryQueue, OutboxCommitExt, OutboxMessage,
    OutboxWorkerThread, Queueable,
};
use std::sync::mpsc::channel;
use std::thread;
use std::time::Duration;
use super::order::{
    Inventory, InventoryReservedPayload, Order, OrderCreatedPayload, OrderFulfillmentSaga,
    OrderItem, Payment, PaymentSucceededPayload, SagaCompletedPayload, SagaStartedPayload,
};

#[test]
fn distributed_saga_with_threads() {
    // Shared queue - all services publish to and subscribe from this
    let queue = InMemoryQueue::new();

    // Channel to signal saga completion
    let (complete_tx, complete_rx) = channel::<String>();

    // =========================================================================
    // SAGA ORCHESTRATOR THREAD
    // =========================================================================
    let saga_queue = queue.clone();
    let saga_complete_tx = complete_tx.clone();
    let saga_thread = thread::spawn(move || {
        let repo = HashMapRepository::new();
        let worker = OutboxWorkerThread::spawn(
            repo.clone(),
            saga_queue.clone(),
            Duration::from_millis(10),
        );
        let saga_repo = repo.queued().aggregate::<OrderFulfillmentSaga>();

        // Create a bus for this service
        let bus = Bus::from_queue(saga_queue);

        // === Start the saga ===
        let saga_id = "saga-001".to_string();
        let order_id = "order-001".to_string();
        let items = vec![OrderItem {
            sku: "WIDGET-001".to_string(),
            quantity: 5,
            price_cents: 1000,
        }];

        let mut saga = OrderFulfillmentSaga::new();
        saga.start(
            saga_id.clone(),
            order_id.clone(),
            "customer-001".to_string(),
            items.clone(),
            5000,
        );

        let mut outbox = OutboxMessage::encode(
            &format!("{}:started", saga_id),
            "SagaStarted",
            &SagaStartedPayload {
                saga_id: saga_id.clone(),
                order_id: order_id.clone(),
                customer_id: "customer-001".to_string(),
                items,
                total_cents: 5000,
            },
        )
        .unwrap();
        saga_repo.outbox(&mut outbox).commit(&mut saga).unwrap();

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
                            let mut saga = saga_repo.get(&saga_id).unwrap().unwrap();
                            saga.inventory_reserved();
                            saga_repo.commit(&mut saga).unwrap();
                        }
                    }
                    "PaymentSucceeded" => {
                        let data: PaymentSucceededPayload = event.decode().unwrap();
                        if data.order_id == order_id {
                            println!("[Saga Orchestrator] Payment succeeded, advancing saga...");
                            let mut saga = saga_repo.get(&saga_id).unwrap().unwrap();
                            saga.payment_succeeded();
                            saga_repo.commit(&mut saga).unwrap();
                        }
                    }
                    "OrderCompleted" => {
                        let data: PaymentSucceededPayload = event.decode().unwrap();
                        if data.order_id == order_id {
                            println!("[Saga Orchestrator] Order completed, completing saga...");
                            let mut saga = saga_repo.get(&saga_id).unwrap().unwrap();
                            saga.complete();

                            let mut outbox = OutboxMessage::encode(
                                &format!("{}:completed", saga_id),
                                "SagaCompleted",
                                &SagaCompletedPayload {
                                    saga_id: saga_id.clone(),
                                    order_id: order_id.clone(),
                                },
                            )
                            .unwrap();
                            saga_repo.outbox(&mut outbox).commit(&mut saga).unwrap();

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

        let final_saga = saga_repo.peek(&saga_id).unwrap().unwrap();
        println!(
            "[Saga Orchestrator] Final saga status: {:?}",
            final_saga.status()
        );

        worker.stop();
    });

    // =========================================================================
    // ORDER SERVICE THREAD
    // =========================================================================
    let order_queue = queue.clone();
    let order_thread = thread::spawn(move || {
        let repo = HashMapRepository::new();
        let worker = OutboxWorkerThread::spawn(
            repo.clone(),
            order_queue.clone(),
            Duration::from_millis(10),
        );
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
                        let data: SagaStartedPayload = event.decode().unwrap();
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
                payment_repo.outbox(&mut outbox).commit(&mut payment).unwrap();

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
    saga_thread.join().expect("Saga thread panicked");
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
