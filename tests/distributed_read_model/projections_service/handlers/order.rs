//! Projects order events into `orders` + `order_lines`. The order snapshot is
//! the desired state, so `save_changes` collection sync reflects added/changed/
//! removed lines as inserts/patches/deletes. A monotonic `source_version` guard
//! ignores stale snapshots under out-of-order delivery. Owns only `orders` and
//! `order_lines` (never `order_fulfillment_steps`), so it includes `lines` only.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use sourced_rust::bus::Event;
use sourced_rust::microsvc::{Context, HandlerError};
use sourced_rust::{InMemoryReadModelStore, ReadModelUnitOfWorkExt};

use crate::order_service::OrderSnapshot;
use crate::read_models::{order_key, OrderLineView, OrderView};

pub const CONSUMER: &str = "order-detail-projection";
pub const EVENTS: &[&str] = &[
    "order.placed",
    "order.line_added",
    "order.line_quantity_changed",
    "order.line_removed",
    "order.submitted",
    "order.confirmed",
    "order.cancelled",
];

pub fn guard(ctx: &Context<InMemoryReadModelStore>) -> bool {
    ctx.has_fields(&["id", "event_type", "payload"])
}

pub fn handle(ctx: &Context<InMemoryReadModelStore>) -> Result<Value, HandlerError> {
    let event = super::event(ctx)?;
    let snapshot: OrderSnapshot = event
        .decode()
        .map_err(|err| HandlerError::DecodeFailed(format!("order snapshot: {err}")))?;
    let version = event_version(&event)?;
    let desired = desired_order_view(&snapshot, version);

    let mut session = ctx.repo().session();
    let existing = session
        .load::<OrderView>(order_key(&desired.order_id))
        .include("lines")
        .one()
        .map_err(super::read_model_error)?;

    match existing {
        Some(current) if current.data.source_version >= version => {}
        Some(_) => {
            session
                .save_changes(desired)
                .map_err(super::read_model_error)?;
        }
        None => {
            session.save(&desired).map_err(super::read_model_error)?;
            for line in &desired.lines {
                session.save(line).map_err(super::read_model_error)?;
            }
        }
    }

    session.mark_processed(CONSUMER, &event.id);
    session.commit().map_err(super::read_model_error)?;

    Ok(json!({ "event_id": event.id }))
}

fn desired_order_view(snapshot: &OrderSnapshot, version: i64) -> OrderView {
    let lines: Vec<OrderLineView> = snapshot
        .lines
        .iter()
        .map(|line| OrderLineView {
            order_id: snapshot.id.clone(),
            sku: line.sku.clone(),
            product_id: line.product_id.clone(),
            quantity: line.quantity,
            line_total_cents: line.unit_cents * line.quantity,
            product: None,
        })
        .collect();
    let total_cents = lines.iter().map(|line| line.line_total_cents).sum();

    let mut metadata = BTreeMap::new();
    metadata.insert("source".to_string(), "order-service".to_string());
    metadata.insert("line_count".to_string(), lines.len().to_string());

    OrderView {
        order_id: snapshot.id.clone(),
        customer: snapshot.customer.clone(),
        status: snapshot.status.clone(),
        source_version: version,
        total_cents,
        metadata,
        lines,
        // Owned by the fulfillment projector; the order projector never includes
        // or reconciles this relationship, so leaving it empty is correct.
        fulfillment_steps: Vec::new(),
    }
}

/// The aggregate version is the trailing segment of the outbox event id
/// (`outbox:<aggregate-id>:<event-type>:<version>`).
pub(super) fn event_version(event: &Event) -> Result<i64, HandlerError> {
    let raw = event
        .id
        .rsplit(':')
        .next()
        .ok_or_else(|| HandlerError::Rejected("event id is missing a version".to_string()))?;

    raw.parse().map_err(|_| {
        HandlerError::Rejected(format!(
            "event id {} should end with a numeric aggregate version",
            event.id
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_version_parses_trailing_outbox_segment() {
        let event = Event::with_string_payload(
            "outbox:order-1:order.line_added:42",
            "order.line_added",
            "{}",
        );

        assert_eq!(event_version(&event).unwrap(), 42);
    }

    #[test]
    fn event_version_rejects_malformed_outbox_segment() {
        let event = Event::with_string_payload(
            "outbox:order-1:order.line_added:bad",
            "order.line_added",
            "{}",
        );

        let err = event_version(&event).unwrap_err();

        assert!(
            matches!(err, HandlerError::Rejected(message) if message.contains("numeric aggregate version"))
        );
    }
}
