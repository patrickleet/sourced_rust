//! Distributed Saga with Outbox Pattern
//!
//! Demonstrates a distributed system where each service runs in its own thread:
//! - Each service has its own repository and outbox worker
//! - Services communicate ONLY via the shared queue
//! - No direct calls between services
//!
//! ```text
//!  ┌─────────────────────────────────────────────────────────────┐
//!  │                   Shared Queue (InMemoryQueue)              │
//!  │              thread-safe via Arc<RwLock<...>>               │
//!  └─────────────────────────────────────────────────────────────┘
//!        ↑↓                    ↑↓                    ↑↓
//!  ┌───────────┐         ┌───────────┐         ┌───────────┐
//!  │  Order    │         │ Inventory │         │  Payment  │
//!  │  Thread   │         │  Thread   │         │  Thread   │
//!  │           │         │           │         │           │
//!  │ repo +    │         │ repo +    │         │ repo +    │
//!  │ worker    │         │ worker    │         │ worker    │
//!  └───────────┘         └───────────┘         └───────────┘
//! ```

mod support;

use serde::{Deserialize, Serialize};
use sourced_rust::{
    bus::Subscriber,
    AggregateBuilder, HashMapRepository, OutboxCommitExt, OutboxMessage, Queueable,
};
use std::sync::mpsc::channel;
use std::thread;
use std::time::Duration;
use support::in_memory_queue::InMemoryQueue;
use support::order::{Inventory, Order, OrderItem, Payment};
use support::outbox_worker_thread::OutboxWorkerThread;

// ============================================================================
// Event Payloads (shared contract between services)
// ============================================================================

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OrderCreatedPayload {
    pub order_id: String,
    pub customer_id: String,
    pub items: Vec<OrderItem>,
    pub total_cents: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InventoryReservedPayload {
    pub order_id: String,
    pub sku: String,
    pub quantity: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PaymentSucceededPayload {
    pub order_id: String,
    pub payment_id: String,
}

// ============================================================================
// Test
// ============================================================================

#[test]
fn distributed_saga_with_threads() {
    // Shared queue - all services publish to and subscribe from this
    let queue = InMemoryQueue::new();

    // Channel to signal saga completion
    let (complete_tx, complete_rx) = channel::<String>();

    // =========================================================================
    // ORDER SERVICE THREAD
    // =========================================================================
    let order_queue = queue.clone();
    let order_complete_tx = complete_tx.clone();
    let order_thread = thread::spawn(move || {
        // Each service has its own repository
        let repo = HashMapRepository::new();

        // Each service has its own outbox worker that publishes to the shared queue
        let worker = OutboxWorkerThread::spawn(
            repo.clone(),
            order_queue.clone(),
            Duration::from_millis(10),
        );

        let order_repo = repo.queued().aggregate::<Order>();

        // === Create the initial order ===
        let items = vec![OrderItem {
            sku: "WIDGET-001".to_string(),
            quantity: 5,
            price_cents: 1000,
        }];

        let mut order = Order::new();
        order.create("order-001".to_string(), "customer-001".to_string(), items.clone());

        let payload = serde_json::to_string(&OrderCreatedPayload {
            order_id: "order-001".to_string(),
            customer_id: "customer-001".to_string(),
            items,
            total_cents: 5000,
        })
        .unwrap();

        let mut outbox = OutboxMessage::create(
            "order-001:created",
            "OrderCreated",
            payload.into_bytes(),
        );
        order_repo.outbox(&mut outbox).commit(&mut order).unwrap();

        println!("[Order Service] Created order-001, waiting for PaymentSucceeded...");

        // === Listen for PaymentSucceeded to complete the order ===
        let subscriber = order_queue.new_subscriber();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);

        while std::time::Instant::now() < deadline {
            if let Ok(Some(event)) = subscriber.poll(100) {
                if event.event_type == "PaymentSucceeded" {
                    let data: PaymentSucceededPayload =
                        serde_json::from_str(event.payload_str().unwrap()).unwrap();

                    if data.order_id == "order-001" {
                        println!("[Order Service] Received PaymentSucceeded, completing order...");

                        let mut order = order_repo.get("order-001").unwrap().unwrap();
                        order.mark_inventory_reserved();
                        order.mark_payment_processed();
                        order.complete();

                        let payload = serde_json::to_string(&data).unwrap();
                        let mut outbox = OutboxMessage::create(
                            "order-001:completed",
                            "OrderCompleted",
                            payload.into_bytes(),
                        );
                        order_repo.outbox(&mut outbox).commit(&mut order).unwrap();

                        // Wait for outbox worker to publish
                        thread::sleep(Duration::from_millis(50));

                        println!("[Order Service] Order completed!");
                        order_complete_tx.send(data.order_id).unwrap();
                        break;
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
        let worker = OutboxWorkerThread::spawn(
            repo.clone(),
            inventory_queue.clone(),
            Duration::from_millis(10),
        );
        let inventory_repo = repo.queued().aggregate::<Inventory>();

        // Initialize inventory first
        let mut inv = Inventory::new();
        inv.initialize("WIDGET-001".to_string(), 100);
        inventory_repo.commit(&mut inv).unwrap();

        println!("[Inventory Service] Initialized with 100 WIDGET-001, waiting for OrderCreated...");

        // Listen for OrderCreated events
        let subscriber = inventory_queue.new_subscriber();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);

        while std::time::Instant::now() < deadline {
            if let Ok(Some(event)) = subscriber.poll(100) {
                if event.event_type == "OrderCreated" {
                    let data: OrderCreatedPayload =
                        serde_json::from_str(event.payload_str().unwrap()).unwrap();

                    if data.order_id == "order-001" {
                        println!("[Inventory Service] Received OrderCreated, reserving inventory...");

                        let item = &data.items[0];
                        let mut inv = inventory_repo.get(&item.sku).unwrap().unwrap();

                        if inv.can_reserve(item.quantity) {
                            inv.reserve(data.order_id.clone(), item.quantity);

                            let payload = serde_json::to_string(&InventoryReservedPayload {
                                order_id: data.order_id.clone(),
                                sku: item.sku.clone(),
                                quantity: item.quantity,
                            })
                            .unwrap();

                            let mut outbox = OutboxMessage::create(
                                "order-001:reserved",
                                "InventoryReserved",
                                payload.into_bytes(),
                            );
                            inventory_repo.outbox(&mut outbox).commit(&mut inv).unwrap();

                            println!("[Inventory Service] Reserved {} units", item.quantity);

                            // Give the outbox worker time to publish
                            thread::sleep(Duration::from_millis(50));
                        }
                        break;
                    }
                }
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

        println!("[Payment Service] Waiting for InventoryReserved...");

        // Listen for InventoryReserved events
        let subscriber = payment_queue.new_subscriber();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);

        while std::time::Instant::now() < deadline {
            if let Ok(Some(event)) = subscriber.poll(100) {
                if event.event_type == "InventoryReserved" {
                    let data: InventoryReservedPayload =
                        serde_json::from_str(event.payload_str().unwrap()).unwrap();

                    if data.order_id == "order-001" {
                        println!("[Payment Service] Received InventoryReserved, processing payment...");

                        let mut payment = Payment::new();
                        let payment_id = format!("pay-{}", data.order_id);
                        payment.initiate(payment_id.clone(), data.order_id.clone(), 5000);
                        payment.authorize("txn-123".to_string());
                        payment.capture();

                        let payload = serde_json::to_string(&PaymentSucceededPayload {
                            order_id: data.order_id.clone(),
                            payment_id,
                        })
                        .unwrap();

                        let mut outbox = OutboxMessage::create(
                            "order-001:paid",
                            "PaymentSucceeded",
                            payload.into_bytes(),
                        );
                        payment_repo.outbox(&mut outbox).commit(&mut payment).unwrap();

                        println!("[Payment Service] Payment succeeded!");

                        // Give the outbox worker time to publish
                        thread::sleep(Duration::from_millis(50));
                        break;
                    }
                }
            }
        }

        worker.stop();
    });

    // =========================================================================
    // WAIT FOR SAGA COMPLETION
    // =========================================================================

    let completed_order = complete_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("Saga should complete within 10 seconds");

    assert_eq!(completed_order, "order-001");

    // Join all threads
    order_thread.join().expect("Order thread panicked");
    inventory_thread.join().expect("Inventory thread panicked");
    payment_thread.join().expect("Payment thread panicked");

    // Verify the event flow
    let event_types = queue.event_types();
    println!("\nEvent flow: {:?}", event_types);

    assert!(event_types.contains(&"OrderCreated".to_string()));
    assert!(event_types.contains(&"InventoryReserved".to_string()));
    assert!(event_types.contains(&"PaymentSucceeded".to_string()));
    assert!(event_types.contains(&"OrderCompleted".to_string()));
}
