use serde::{Deserialize, Serialize};
use sourced_rust::{digest, Entity};

/// Payment status
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PaymentStatus {
    Pending,
    Authorized,
    Captured,
    Failed,
    Refunded,
}

impl Default for PaymentStatus {
    fn default() -> Self {
        PaymentStatus::Pending
    }
}

/// Payment aggregate - represents a payment attempt for an order
#[derive(Default)]
pub struct Payment {
    pub entity: Entity,
    order_id: String,
    amount_cents: u32,
    status: PaymentStatus,
    failure_reason: Option<String>,
    transaction_id: Option<String>,
}

#[allow(dead_code)]
impl Payment {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn order_id(&self) -> &str {
        &self.order_id
    }

    pub fn amount_cents(&self) -> u32 {
        self.amount_cents
    }

    pub fn status(&self) -> PaymentStatus {
        self.status
    }

    pub fn is_successful(&self) -> bool {
        matches!(self.status, PaymentStatus::Authorized | PaymentStatus::Captured)
    }

    pub fn failure_reason(&self) -> Option<&str> {
        self.failure_reason.as_deref()
    }

    #[digest("PaymentInitiated")]
    pub fn initiate(&mut self, id: String, order_id: String, amount_cents: u32) {
        self.entity.set_id(&id);
        self.order_id = order_id;
        self.amount_cents = amount_cents;
        self.status = PaymentStatus::Pending;
    }

    #[digest("PaymentAuthorized", when = self.status == PaymentStatus::Pending)]
    pub fn authorize(&mut self, transaction_id: String) {
        self.status = PaymentStatus::Authorized;
        self.transaction_id = Some(transaction_id);
    }

    #[digest("PaymentCaptured", when = self.status == PaymentStatus::Authorized)]
    pub fn capture(&mut self) {
        self.status = PaymentStatus::Captured;
    }

    #[digest("PaymentFailed", when = self.status == PaymentStatus::Pending || self.status == PaymentStatus::Authorized)]
    pub fn fail(&mut self, reason: String) {
        self.status = PaymentStatus::Failed;
        self.failure_reason = Some(reason);
    }

    #[digest("PaymentRefunded", when = self.status == PaymentStatus::Captured)]
    pub fn refund(&mut self) {
        self.status = PaymentStatus::Refunded;
    }

    pub fn snapshot(&self) -> PaymentSnapshot {
        PaymentSnapshot {
            id: self.entity.id().to_string(),
            order_id: self.order_id.clone(),
            amount_cents: self.amount_cents,
            status: self.status,
            failure_reason: self.failure_reason.clone(),
            transaction_id: self.transaction_id.clone(),
        }
    }
}

sourced_rust::aggregate!(Payment, entity {
    "PaymentInitiated"(id, order_id, amount_cents) => initiate,
    "PaymentAuthorized"(transaction_id) => authorize,
    "PaymentCaptured"() => capture(),
    "PaymentFailed"(reason) => fail,
    "PaymentRefunded"() => refund(),
});

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PaymentSnapshot {
    pub id: String,
    pub order_id: String,
    pub amount_cents: u32,
    pub status: PaymentStatus,
    pub failure_reason: Option<String>,
    pub transaction_id: Option<String>,
}
