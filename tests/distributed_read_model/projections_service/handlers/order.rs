//! Projects order events into `orders` + `order_lines`. The order snapshot is
//! the desired state, so `save_changes` collection sync reflects added/changed/
//! removed lines as inserts/patches/deletes. A monotonic `source_version` guard
//! ignores stale snapshots under out-of-order delivery. Owns only `orders` and
//! `order_lines` (never `order_fulfillment_steps`), so it includes `lines` only.

use std::collections::BTreeMap;

use sourced_rust::bus::Event;
use sourced_rust::{InMemoryReadModelStore, ReadModelCommitOutcome, ReadModelUnitOfWorkExt};

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

pub fn handle(store: &InMemoryReadModelStore, event: &Event) -> ReadModelCommitOutcome {
    let snapshot: OrderSnapshot = event.decode().expect("order snapshot should decode");
    let version = event_version(event);
    let desired = desired_order_view(&snapshot, version);

    let mut session = store.session();
    let existing = session
        .load::<OrderView>(order_key(&desired.order_id))
        .include("lines")
        .one()
        .expect("order load should succeed");

    match existing {
        Some(current) if current.data.source_version >= version => {}
        Some(_) => {
            session
                .save_changes(desired)
                .expect("order save_changes should stage");
        }
        None => {
            session
                .save(&desired)
                .expect("order root save should stage");
            for line in &desired.lines {
                session.save(line).expect("order line save should stage");
            }
        }
    }

    session.mark_processed(CONSUMER, &event.id);
    session.commit().expect("order projection should commit")
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
pub(super) fn event_version(event: &Event) -> i64 {
    event
        .id
        .rsplit(':')
        .next()
        .and_then(|raw| raw.parse().ok())
        .unwrap_or(0)
}
