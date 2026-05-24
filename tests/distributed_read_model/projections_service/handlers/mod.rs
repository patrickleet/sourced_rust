//! Projection handlers. Commands and events are both just messages, so the
//! projection service dispatches each event by type to the handler that owns the
//! matching read-model rows.

pub mod fulfillment;
pub mod order;
pub mod product;

use sourced_rust::bus::Event;
use sourced_rust::{InMemoryReadModelStore, ReadModelCommitOutcome};

/// Every event type any projection handler consumes.
pub fn event_types() -> Vec<&'static str> {
    let mut types = Vec::new();
    types.extend_from_slice(product::EVENTS);
    types.extend_from_slice(order::EVENTS);
    types.extend_from_slice(fulfillment::EVENTS);
    types
}

/// Route one event to the handler that owns its rows.
pub fn project(store: &InMemoryReadModelStore, event: &Event) -> Option<ReadModelCommitOutcome> {
    let event_type = event.event_type.as_str();
    if product::EVENTS.contains(&event_type) {
        Some(product::handle(store, event))
    } else if order::EVENTS.contains(&event_type) {
        Some(order::handle(store, event))
    } else if fulfillment::EVENTS.contains(&event_type) {
        Some(fulfillment::handle(store, event))
    } else {
        None
    }
}
