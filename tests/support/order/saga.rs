use serde::{Deserialize, Serialize};
use sourced_rust::{digest, Entity};

use super::order::OrderItem;

/// Saga status - tracks the overall state machine
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SagaStatus {
    Started,
    InventoryReserved,
    PaymentProcessed,
    Completed,
    Compensating,
    Failed,
}

impl Default for SagaStatus {
    fn default() -> Self {
        SagaStatus::Started
    }
}

/// Tracks what compensating actions are needed
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CompensationState {
    pub inventory_reserved: bool,
    pub payment_processed: bool,
}

/// OrderFulfillmentSaga - an event-sourced aggregate that coordinates
/// the order fulfillment process across multiple aggregates.
///
/// Flow:
/// 1. Start saga with order details
/// 2. Reserve inventory for each item
/// 3. Process payment
/// 4. Mark order as completed
///
/// On failure at any step, compensate by:
/// - Releasing inventory reservations
/// - Refunding payment (if processed)
/// - Cancelling order
#[derive(Default)]
pub struct OrderFulfillmentSaga {
    pub entity: Entity,
    order_id: String,
    customer_id: String,
    items: Vec<OrderItem>,
    total_cents: u32,
    status: SagaStatus,
    compensation: CompensationState,
    failure_reason: Option<String>,
}

impl OrderFulfillmentSaga {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn order_id(&self) -> &str {
        &self.order_id
    }

    pub fn customer_id(&self) -> &str {
        &self.customer_id
    }

    pub fn items(&self) -> &[OrderItem] {
        &self.items
    }

    pub fn total_cents(&self) -> u32 {
        self.total_cents
    }

    pub fn status(&self) -> SagaStatus {
        self.status
    }

    pub fn compensation(&self) -> &CompensationState {
        &self.compensation
    }

    pub fn failure_reason(&self) -> Option<&str> {
        self.failure_reason.as_deref()
    }

    pub fn is_complete(&self) -> bool {
        matches!(self.status, SagaStatus::Completed | SagaStatus::Failed)
    }

    pub fn needs_inventory_compensation(&self) -> bool {
        self.status == SagaStatus::Compensating && self.compensation.inventory_reserved
    }

    pub fn needs_payment_compensation(&self) -> bool {
        self.status == SagaStatus::Compensating && self.compensation.payment_processed
    }

    // === Saga Commands ===

    #[digest("SagaStarted")]
    pub fn start(
        &mut self,
        saga_id: String,
        order_id: String,
        customer_id: String,
        items: Vec<OrderItem>,
        total_cents: u32,
    ) {
        self.entity.set_id(&saga_id);
        self.order_id = order_id;
        self.customer_id = customer_id;
        self.items = items;
        self.total_cents = total_cents;
        self.status = SagaStatus::Started;
    }

    #[digest("InventoryReservationSucceeded", when = self.status == SagaStatus::Started)]
    pub fn inventory_reserved(&mut self) {
        self.status = SagaStatus::InventoryReserved;
        self.compensation.inventory_reserved = true;
    }

    #[digest("PaymentSucceeded", when = self.status == SagaStatus::InventoryReserved)]
    pub fn payment_succeeded(&mut self) {
        self.status = SagaStatus::PaymentProcessed;
        self.compensation.payment_processed = true;
    }

    #[digest("SagaCompleted", when = self.status == SagaStatus::PaymentProcessed)]
    pub fn complete(&mut self) {
        self.status = SagaStatus::Completed;
    }

    // === Failure and Compensation ===

    #[digest("StepFailed", when = !self.is_complete())]
    pub fn step_failed(&mut self, step: String, reason: String) {
        self.status = SagaStatus::Compensating;
        self.failure_reason = Some(format!("{}: {}", step, reason));
    }

    #[digest("InventoryCompensated", when = self.needs_inventory_compensation())]
    pub fn inventory_compensated(&mut self) {
        self.compensation.inventory_reserved = false;
    }

    #[digest("PaymentCompensated", when = self.needs_payment_compensation())]
    pub fn payment_compensated(&mut self) {
        self.compensation.payment_processed = false;
    }

    #[digest("SagaFailed", when = self.status == SagaStatus::Compensating && !self.compensation.inventory_reserved && !self.compensation.payment_processed)]
    pub fn mark_failed(&mut self) {
        self.status = SagaStatus::Failed;
    }

    pub fn snapshot(&self) -> OrderFulfillmentSagaSnapshot {
        OrderFulfillmentSagaSnapshot {
            id: self.entity.id().to_string(),
            order_id: self.order_id.clone(),
            customer_id: self.customer_id.clone(),
            items: self.items.clone(),
            total_cents: self.total_cents,
            status: self.status,
            compensation: self.compensation.clone(),
            failure_reason: self.failure_reason.clone(),
        }
    }
}

sourced_rust::aggregate!(OrderFulfillmentSaga, entity {
    "SagaStarted"(saga_id, order_id, customer_id, items, total_cents) => start,
    "InventoryReservationSucceeded"() => inventory_reserved(),
    "PaymentSucceeded"() => payment_succeeded(),
    "SagaCompleted"() => complete(),
    "StepFailed"(step, reason) => step_failed,
    "InventoryCompensated"() => inventory_compensated(),
    "PaymentCompensated"() => payment_compensated(),
    "SagaFailed"() => mark_failed(),
});

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OrderFulfillmentSagaSnapshot {
    pub id: String,
    pub order_id: String,
    pub customer_id: String,
    pub items: Vec<OrderItem>,
    pub total_cents: u32,
    pub status: SagaStatus,
    pub compensation: CompensationState,
    pub failure_reason: Option<String>,
}
