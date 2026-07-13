//! Orders bounded context — WorkshopOrder aggregate.
//!
//! Spec: [[specs/workshop-domain]] (distributed tests fixture).

use distributed::{sourced, Entity};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum OrdersError {
    #[error("order already placed")]
    AlreadyPlaced,
    #[error("order not open")]
    NotOpen,
    #[error("empty order id")]
    EmptyId,
    #[error("empty product id")]
    EmptyProduct,
    #[error("quantity must be positive")]
    InvalidQty,
    #[error(transparent)]
    Event(#[from] distributed::EventRecordError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum OrderStatus {
    #[default]
    Draft,
    Placed,
    Fulfilled,
    Cancelled,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkshopOrder {
    #[serde(skip, default)]
    pub entity: Entity,
    pub order_id: String,
    pub product_id: String,
    pub customer_id: String,
    pub quantity: u32,
    pub status: OrderStatus,
}

#[sourced(entity, events = "WorkshopOrderEvent", aggregate_type = "workshop_order")]
impl WorkshopOrder {
    pub fn is_placed(&self) -> bool {
        matches!(self.status, OrderStatus::Placed)
    }

    pub fn place(
        &mut self,
        order_id: impl Into<String>,
        product_id: impl Into<String>,
        customer_id: impl Into<String>,
        quantity: u32,
    ) -> Result<(), OrdersError> {
        let order_id = order_id.into();
        let product_id = product_id.into();
        let customer_id = customer_id.into();
        if !matches!(self.status, OrderStatus::Draft) && !self.order_id.is_empty() {
            return Err(OrdersError::AlreadyPlaced);
        }
        if order_id.trim().is_empty() {
            return Err(OrdersError::EmptyId);
        }
        if product_id.trim().is_empty() {
            return Err(OrdersError::EmptyProduct);
        }
        if quantity == 0 {
            return Err(OrdersError::InvalidQty);
        }
        self.record_placed(order_id, product_id, customer_id, quantity)?;
        Ok(())
    }

    #[event("workshop_order.placed")]
    fn record_placed(
        &mut self,
        order_id: String,
        product_id: String,
        customer_id: String,
        quantity: u32,
    ) {
        self.entity.set_id(&order_id);
        self.order_id = order_id;
        self.product_id = product_id;
        self.customer_id = customer_id;
        self.quantity = quantity;
        self.status = OrderStatus::Placed;
    }

    pub fn fulfill(&mut self) -> Result<(), OrdersError> {
        if !self.is_placed() {
            return Err(OrdersError::NotOpen);
        }
        self.record_fulfilled()?;
        Ok(())
    }

    #[event("workshop_order.fulfilled")]
    fn record_fulfilled(&mut self) {
        self.status = OrderStatus::Fulfilled;
    }

    pub fn cancel(&mut self) -> Result<(), OrdersError> {
        if !self.is_placed() {
            return Err(OrdersError::NotOpen);
        }
        self.record_cancelled()?;
        Ok(())
    }

    #[event("workshop_order.cancelled")]
    fn record_cancelled(&mut self) {
        self.status = OrderStatus::Cancelled;
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkshopOrderPlaced {
    pub order_id: String,
    pub product_id: String,
    pub customer_id: String,
    pub quantity: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkshopOrderFulfilled {
    pub order_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkshopOrderCancelled {
    pub order_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn place_fulfill() {
        let mut o = WorkshopOrder::default();
        o.place("o1", "p1", "c1", 2).unwrap();
        assert!(o.is_placed());
        o.fulfill().unwrap();
        assert_eq!(o.status, OrderStatus::Fulfilled);
    }
}
