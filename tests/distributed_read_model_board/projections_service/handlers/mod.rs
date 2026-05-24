//! Projection handlers. Each event is a message dispatched by type to the
//! handler that owns the matching read-model rows.

pub mod board;

use sourced_rust::bus::Event;
use sourced_rust::{InMemoryReadModelStore, ReadModelCommitOutcome};

/// Every event type any projection handler consumes.
pub fn event_types() -> Vec<&'static str> {
    board::EVENTS.to_vec()
}

/// Route one event to the handler that owns its rows.
pub fn project(store: &InMemoryReadModelStore, event: &Event) -> Option<ReadModelCommitOutcome> {
    if board::EVENTS.contains(&event.event_type.as_str()) {
        Some(board::handle(store, event))
    } else {
        None
    }
}
