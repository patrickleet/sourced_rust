//! Saga Pattern Example
//!
//! This module demonstrates the saga pattern using event sourcing.
//! A saga is an event-sourced aggregate that coordinates a multi-step
//! business process across multiple aggregates, with compensation
//! (rollback) capabilities when steps fail.
//!
//! ## The Order Fulfillment Saga
//!
//! This example implements an order fulfillment process:
//!
//! 1. **Start**: Saga receives order details
//! 2. **Reserve Inventory**: Reserve items from inventory
//! 3. **Process Payment**: Charge the customer
//! 4. **Complete**: Mark order as fulfilled
//!
//! If any step fails, the saga compensates by:
//! - Releasing inventory reservations
//! - Refunding payments
//! - Cancelling the order
//!
//! ## Key Concepts
//!
//! - **Saga as Aggregate**: The saga itself is event-sourced, so its state
//!   can be rebuilt from its event history
//! - **Compensation State**: Tracks what actions need to be undone on failure
//! - **Idempotency**: Each step can be retried safely

mod support;

use sourced_rust::{AggregateBuilder, HashMapRepository};
use support::order::{
    Inventory, Order, OrderFulfillmentSaga, OrderItem, Payment, PaymentStatus, SagaStatus,
};

#[test]
fn saga_happy_path_completes_order() {
    // Set up repositories for each aggregate type
    let order_repo = HashMapRepository::new().aggregate::<Order>();
    let inventory_repo = HashMapRepository::new().aggregate::<Inventory>();
    let payment_repo = HashMapRepository::new().aggregate::<Payment>();
    let saga_repo = HashMapRepository::new().aggregate::<OrderFulfillmentSaga>();

    // === Setup: Initialize inventory ===
    let mut widget_inventory = Inventory::new();
    widget_inventory.initialize("WIDGET-001".to_string(), 100);
    inventory_repo.commit(&mut widget_inventory).unwrap();

    // === Step 1: Create Order ===
    let order_id = "order-123".to_string();
    let items = vec![OrderItem {
        sku: "WIDGET-001".to_string(),
        quantity: 5,
        price_cents: 1000,
    }];

    let mut order = Order::new();
    order.create(order_id.clone(), "customer-456".to_string(), items.clone());
    order_repo.commit(&mut order).unwrap();

    // === Step 2: Start Saga ===
    let mut saga = OrderFulfillmentSaga::new();
    saga.start(
        "saga-123".to_string(),
        order_id.clone(),
        "customer-456".to_string(),
        items,
        5000, // 5 widgets * 1000 cents
    );
    assert_eq!(saga.status(), SagaStatus::Started);
    saga_repo.commit(&mut saga).unwrap();

    // === Step 3: Reserve Inventory ===
    let mut inventory = inventory_repo.get("WIDGET-001").unwrap().unwrap();
    assert!(inventory.can_reserve(5));
    inventory.reserve(order_id.clone(), 5);
    inventory_repo.commit(&mut inventory).unwrap();

    // Update saga state
    let mut saga = saga_repo.get("saga-123").unwrap().unwrap();
    saga.inventory_reserved();
    assert_eq!(saga.status(), SagaStatus::InventoryReserved);
    assert!(saga.compensation().inventory_reserved);
    saga_repo.commit(&mut saga).unwrap();

    // Update order state
    let mut order = order_repo.get(&order_id).unwrap().unwrap();
    order.mark_inventory_reserved();
    order_repo.commit(&mut order).unwrap();

    // === Step 4: Process Payment ===
    let mut payment = Payment::new();
    payment.initiate("payment-789".to_string(), order_id.clone(), 5000);
    payment.authorize("txn-abc123".to_string());
    payment.capture();
    assert!(payment.is_successful());
    payment_repo.commit(&mut payment).unwrap();

    // Update saga state
    let mut saga = saga_repo.get("saga-123").unwrap().unwrap();
    saga.payment_succeeded();
    assert_eq!(saga.status(), SagaStatus::PaymentProcessed);
    assert!(saga.compensation().payment_processed);
    saga_repo.commit(&mut saga).unwrap();

    // Update order state
    let mut order = order_repo.get(&order_id).unwrap().unwrap();
    order.mark_payment_processed();
    order_repo.commit(&mut order).unwrap();

    // === Step 5: Complete Saga ===
    let mut saga = saga_repo.get("saga-123").unwrap().unwrap();
    saga.complete();
    assert_eq!(saga.status(), SagaStatus::Completed);
    saga_repo.commit(&mut saga).unwrap();

    // Commit the inventory reservation (no longer reversible)
    let mut inventory = inventory_repo.get("WIDGET-001").unwrap().unwrap();
    inventory.commit_reservation(order_id.clone());
    inventory_repo.commit(&mut inventory).unwrap();

    // Complete the order
    let mut order = order_repo.get(&order_id).unwrap().unwrap();
    order.complete();
    order_repo.commit(&mut order).unwrap();

    // === Verify Final State ===
    let final_saga = saga_repo.get("saga-123").unwrap().unwrap();
    assert_eq!(final_saga.status(), SagaStatus::Completed);
    assert!(final_saga.is_complete());

    let final_order = order_repo.get(&order_id).unwrap().unwrap();
    assert_eq!(
        final_order.status(),
        support::order::OrderStatus::Completed
    );

    let final_inventory = inventory_repo.get("WIDGET-001").unwrap().unwrap();
    assert_eq!(final_inventory.available(), 95); // 100 - 5
    assert_eq!(final_inventory.reserved(), 0);
    assert!(final_inventory.reservation_for_order(&order_id).is_none());
}

#[test]
fn saga_compensates_on_payment_failure() {
    // Set up repositories
    let order_repo = HashMapRepository::new().aggregate::<Order>();
    let inventory_repo = HashMapRepository::new().aggregate::<Inventory>();
    let payment_repo = HashMapRepository::new().aggregate::<Payment>();
    let saga_repo = HashMapRepository::new().aggregate::<OrderFulfillmentSaga>();

    // === Setup ===
    let mut widget_inventory = Inventory::new();
    widget_inventory.initialize("WIDGET-002".to_string(), 50);
    inventory_repo.commit(&mut widget_inventory).unwrap();

    let order_id = "order-fail-456".to_string();
    let items = vec![OrderItem {
        sku: "WIDGET-002".to_string(),
        quantity: 10,
        price_cents: 500,
    }];

    let mut order = Order::new();
    order.create(order_id.clone(), "customer-789".to_string(), items.clone());
    order_repo.commit(&mut order).unwrap();

    // === Start Saga ===
    let mut saga = OrderFulfillmentSaga::new();
    saga.start(
        "saga-fail-456".to_string(),
        order_id.clone(),
        "customer-789".to_string(),
        items,
        5000,
    );
    saga_repo.commit(&mut saga).unwrap();

    // === Reserve Inventory (succeeds) ===
    let mut inventory = inventory_repo.get("WIDGET-002").unwrap().unwrap();
    inventory.reserve(order_id.clone(), 10);
    inventory_repo.commit(&mut inventory).unwrap();

    let mut saga = saga_repo.get("saga-fail-456").unwrap().unwrap();
    saga.inventory_reserved();
    saga_repo.commit(&mut saga).unwrap();

    let mut order = order_repo.get(&order_id).unwrap().unwrap();
    order.mark_inventory_reserved();
    order_repo.commit(&mut order).unwrap();

    // Verify inventory is reserved
    let inventory = inventory_repo.get("WIDGET-002").unwrap().unwrap();
    assert_eq!(inventory.available(), 40); // 50 - 10
    assert_eq!(inventory.reserved(), 10);

    // === Payment Fails ===
    let mut payment = Payment::new();
    payment.initiate("payment-fail-xyz".to_string(), order_id.clone(), 5000);
    payment.fail("Insufficient funds".to_string());
    assert!(!payment.is_successful());
    assert_eq!(payment.status(), PaymentStatus::Failed);
    payment_repo.commit(&mut payment).unwrap();

    // === Saga enters compensation mode ===
    let mut saga = saga_repo.get("saga-fail-456").unwrap().unwrap();
    saga.step_failed("Payment".to_string(), "Insufficient funds".to_string());
    assert_eq!(saga.status(), SagaStatus::Compensating);
    assert!(saga.needs_inventory_compensation());
    assert!(!saga.needs_payment_compensation()); // Payment wasn't successful
    saga_repo.commit(&mut saga).unwrap();

    // === Compensate: Release Inventory ===
    let mut inventory = inventory_repo.get("WIDGET-002").unwrap().unwrap();
    inventory.release_reservation(order_id.clone());
    inventory_repo.commit(&mut inventory).unwrap();

    let mut saga = saga_repo.get("saga-fail-456").unwrap().unwrap();
    saga.inventory_compensated();
    assert!(!saga.needs_inventory_compensation());
    saga_repo.commit(&mut saga).unwrap();

    // === Cancel Order ===
    let mut order = order_repo.get(&order_id).unwrap().unwrap();
    order.cancel("Payment failed: Insufficient funds".to_string());
    order_repo.commit(&mut order).unwrap();

    // === Mark Saga as Failed ===
    let mut saga = saga_repo.get("saga-fail-456").unwrap().unwrap();
    saga.mark_failed();
    assert_eq!(saga.status(), SagaStatus::Failed);
    assert!(saga.is_complete());
    saga_repo.commit(&mut saga).unwrap();

    // === Verify Final State ===
    let final_saga = saga_repo.get("saga-fail-456").unwrap().unwrap();
    assert_eq!(final_saga.status(), SagaStatus::Failed);
    assert_eq!(
        final_saga.failure_reason(),
        Some("Payment: Insufficient funds")
    );

    let final_order = order_repo.get(&order_id).unwrap().unwrap();
    assert_eq!(
        final_order.status(),
        support::order::OrderStatus::Cancelled
    );

    // Inventory should be restored
    let final_inventory = inventory_repo.get("WIDGET-002").unwrap().unwrap();
    assert_eq!(final_inventory.available(), 50); // Back to original
    assert_eq!(final_inventory.reserved(), 0);
}

#[test]
fn saga_compensates_on_inventory_failure() {
    // Set up repositories
    let order_repo = HashMapRepository::new().aggregate::<Order>();
    let inventory_repo = HashMapRepository::new().aggregate::<Inventory>();
    let saga_repo = HashMapRepository::new().aggregate::<OrderFulfillmentSaga>();

    // === Setup: Low inventory ===
    let mut widget_inventory = Inventory::new();
    widget_inventory.initialize("WIDGET-003".to_string(), 5); // Only 5 available
    inventory_repo.commit(&mut widget_inventory).unwrap();

    let order_id = "order-inv-fail-789".to_string();
    let items = vec![OrderItem {
        sku: "WIDGET-003".to_string(),
        quantity: 10, // Trying to order 10
        price_cents: 500,
    }];

    let mut order = Order::new();
    order.create(order_id.clone(), "customer-xyz".to_string(), items.clone());
    order_repo.commit(&mut order).unwrap();

    // === Start Saga ===
    let mut saga = OrderFulfillmentSaga::new();
    saga.start(
        "saga-inv-fail".to_string(),
        order_id.clone(),
        "customer-xyz".to_string(),
        items,
        5000,
    );
    saga_repo.commit(&mut saga).unwrap();

    // === Try to Reserve Inventory (fails - not enough stock) ===
    let inventory = inventory_repo.get("WIDGET-003").unwrap().unwrap();
    assert!(!inventory.can_reserve(10)); // Can't reserve 10 when only 5 available

    // Saga fails at first step
    let mut saga = saga_repo.get("saga-inv-fail").unwrap().unwrap();
    saga.step_failed(
        "Inventory".to_string(),
        "Insufficient stock for WIDGET-003".to_string(),
    );
    assert_eq!(saga.status(), SagaStatus::Compensating);
    // No compensation needed - nothing was reserved yet
    assert!(!saga.needs_inventory_compensation());
    assert!(!saga.needs_payment_compensation());
    saga_repo.commit(&mut saga).unwrap();

    // Cancel order
    let mut order = order_repo.get(&order_id).unwrap().unwrap();
    order.cancel("Insufficient stock".to_string());
    order_repo.commit(&mut order).unwrap();

    // Mark saga as failed (no compensation needed)
    let mut saga = saga_repo.get("saga-inv-fail").unwrap().unwrap();
    saga.mark_failed();
    saga_repo.commit(&mut saga).unwrap();

    // === Verify Final State ===
    let final_saga = saga_repo.get("saga-inv-fail").unwrap().unwrap();
    assert_eq!(final_saga.status(), SagaStatus::Failed);
    assert!(final_saga.is_complete());

    let final_order = order_repo.get(&order_id).unwrap().unwrap();
    assert_eq!(
        final_order.status(),
        support::order::OrderStatus::Cancelled
    );

    // Inventory unchanged
    let final_inventory = inventory_repo.get("WIDGET-003").unwrap().unwrap();
    assert_eq!(final_inventory.available(), 5);
    assert_eq!(final_inventory.reserved(), 0);
}

#[test]
fn saga_is_replayable_from_events() {
    let saga_repo = HashMapRepository::new().aggregate::<OrderFulfillmentSaga>();

    let items = vec![OrderItem {
        sku: "WIDGET-REPLAY".to_string(),
        quantity: 3,
        price_cents: 1500,
    }];

    // Create and progress a saga
    let mut saga = OrderFulfillmentSaga::new();
    saga.start(
        "saga-replay".to_string(),
        "order-replay".to_string(),
        "customer-replay".to_string(),
        items,
        4500,
    );
    saga.inventory_reserved();
    saga.payment_succeeded();

    // Commit to repository
    saga_repo.commit(&mut saga).unwrap();

    // Retrieve and verify state is reconstructed from events
    let restored = saga_repo.get("saga-replay").unwrap().unwrap();

    assert_eq!(restored.order_id(), "order-replay");
    assert_eq!(restored.customer_id(), "customer-replay");
    assert_eq!(restored.total_cents(), 4500);
    assert_eq!(restored.status(), SagaStatus::PaymentProcessed);
    assert!(restored.compensation().inventory_reserved);
    assert!(restored.compensation().payment_processed);

    // Can continue from restored state
    let mut restored = restored;
    restored.complete();
    saga_repo.commit(&mut restored).unwrap();

    let final_saga = saga_repo.get("saga-replay").unwrap().unwrap();
    assert_eq!(final_saga.status(), SagaStatus::Completed);
}

#[test]
fn saga_tracks_compensation_state_correctly() {
    let saga_repo = HashMapRepository::new().aggregate::<OrderFulfillmentSaga>();

    let items = vec![OrderItem {
        sku: "WIDGET-COMP".to_string(),
        quantity: 1,
        price_cents: 100,
    }];

    // Progress saga through multiple steps
    let mut saga = OrderFulfillmentSaga::new();
    saga.start(
        "saga-comp".to_string(),
        "order-comp".to_string(),
        "customer-comp".to_string(),
        items,
        100,
    );

    // Initially no compensation needed
    assert!(!saga.compensation().inventory_reserved);
    assert!(!saga.compensation().payment_processed);

    // After inventory reserved
    saga.inventory_reserved();
    assert!(saga.compensation().inventory_reserved);
    assert!(!saga.compensation().payment_processed);

    // After payment processed
    saga.payment_succeeded();
    assert!(saga.compensation().inventory_reserved);
    assert!(saga.compensation().payment_processed);

    // Fail after both steps completed
    saga.step_failed("FinalStep".to_string(), "Something went wrong".to_string());
    assert_eq!(saga.status(), SagaStatus::Compensating);
    assert!(saga.needs_inventory_compensation());
    assert!(saga.needs_payment_compensation());

    // Compensate payment first
    saga.payment_compensated();
    assert!(saga.needs_inventory_compensation());
    assert!(!saga.needs_payment_compensation());

    // Compensate inventory
    saga.inventory_compensated();
    assert!(!saga.needs_inventory_compensation());
    assert!(!saga.needs_payment_compensation());

    // Now can mark as failed
    saga.mark_failed();
    assert_eq!(saga.status(), SagaStatus::Failed);
    assert!(saga.is_complete());

    // Verify it persists correctly
    saga_repo.commit(&mut saga).unwrap();
    let restored = saga_repo.get("saga-comp").unwrap().unwrap();
    assert_eq!(restored.status(), SagaStatus::Failed);
    assert!(!restored.compensation().inventory_reserved);
    assert!(!restored.compensation().payment_processed);
}
