use serde::{Deserialize, Serialize};
use sourced_rust::{sourced, Entity, Snapshot};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct OrderLineState {
    pub sku: String,
    pub product_id: String,
    pub unit_cents: i64,
    pub quantity: i64,
}

#[derive(Default, Snapshot)]
pub struct Order {
    pub entity: Entity,
    pub customer: String,
    pub status: String,
    pub lines: Vec<OrderLineState>,
}

#[sourced(entity)]
impl Order {
    #[event("OrderPlaced")]
    pub fn place(&mut self, id: String, customer: String) {
        self.entity.set_id(&id);
        self.customer = customer;
        self.status = "open".to_string();
    }

    #[event(
        "LineAdded",
        when = self.status.as_str() == "open" && unit_cents > 0 && quantity > 0
    )]
    pub fn add_line(&mut self, sku: String, product_id: String, unit_cents: i64, quantity: i64) {
        if let Some(line) = self.lines.iter_mut().find(|line| line.sku == sku) {
            line.quantity += quantity;
            line.unit_cents = unit_cents;
        } else {
            self.lines.push(OrderLineState {
                sku,
                product_id,
                unit_cents,
                quantity,
            });
        }
    }

    #[event("LineQuantityChanged", when = self.status.as_str() == "open" && quantity > 0)]
    pub fn change_quantity(&mut self, sku: String, quantity: i64) {
        if let Some(line) = self.lines.iter_mut().find(|line| line.sku == sku) {
            line.quantity = quantity;
        }
    }

    #[event("LineRemoved", when = self.status.as_str() == "open")]
    pub fn remove_line(&mut self, sku: String) {
        self.lines.retain(|line| line.sku != sku);
    }

    #[event("OrderSubmitted", when = self.status.as_str() == "open" && !self.lines.is_empty())]
    pub fn submit(&mut self) {
        self.status = "submitted".to_string();
    }

    #[event("OrderConfirmed", when = self.status.as_str() == "submitted")]
    pub fn confirm(&mut self) {
        self.status = "confirmed".to_string();
    }

    #[event("OrderCancelled", when = self.status.as_str() != "cancelled")]
    pub fn cancel(&mut self) {
        self.status = "cancelled".to_string();
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PlaceOrder {
    pub id: String,
    pub customer: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AddLine {
    pub id: String,
    pub sku: String,
    pub product_id: String,
    pub unit_cents: i64,
    pub quantity: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ChangeQuantity {
    pub id: String,
    pub sku: String,
    pub quantity: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RemoveLine {
    pub id: String,
    pub sku: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SubmitOrder {
    pub id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open_order() -> Order {
        let mut order = Order::default();
        order
            .place("order-1".to_string(), "Ada Lovelace".to_string())
            .unwrap();
        order
    }

    fn submitted_order() -> Order {
        let mut order = open_order();
        order
            .add_line("W".to_string(), "prod-widget".to_string(), 500, 2)
            .unwrap();
        order.submit().unwrap();
        order
    }

    #[test]
    fn add_line_ignores_non_positive_unit_cents() {
        let mut order = open_order();

        order
            .add_line("W".to_string(), "prod-widget".to_string(), 0, 1)
            .unwrap();

        assert!(order.lines.is_empty());
        assert_eq!(order.entity.events().len(), 1);
    }

    #[test]
    fn add_line_ignores_non_positive_quantity() {
        let mut order = open_order();

        order
            .add_line("W".to_string(), "prod-widget".to_string(), 500, 0)
            .unwrap();

        assert!(order.lines.is_empty());
        assert_eq!(order.entity.events().len(), 1);
    }

    #[test]
    fn add_line_ignores_closed_orders() {
        let mut order = submitted_order();

        order
            .add_line("G".to_string(), "prod-gadget".to_string(), 1000, 1)
            .unwrap();

        assert_eq!(order.lines.len(), 1);
        assert_eq!(order.lines[0].sku, "W");
        assert_eq!(order.entity.events().len(), 3);
    }

    #[test]
    fn change_quantity_ignores_closed_orders() {
        let mut order = submitted_order();

        order.change_quantity("W".to_string(), 4).unwrap();

        assert_eq!(order.lines[0].quantity, 2);
        assert_eq!(order.entity.events().len(), 3);
    }

    #[test]
    fn remove_line_ignores_closed_orders() {
        let mut order = submitted_order();

        order.remove_line("W".to_string()).unwrap();

        assert_eq!(order.lines.len(), 1);
        assert_eq!(order.lines[0].sku, "W");
        assert_eq!(order.entity.events().len(), 3);
    }
}
