//! Projects saga progress into `order_fulfillment_steps` (a `has_many` child of
//! `OrderView`). Owns only the steps table — disjoint from the order handler, so
//! there is no optimistic-version contention on the order row.

use sourced_rust::bus::Event;
use sourced_rust::{InMemoryReadModelStore, ReadModelCommitOutcome, ReadModelUnitOfWorkExt};

use crate::fulfillment::{self, event};
use crate::read_models::OrderFulfillmentStepView;

pub const CONSUMER: &str = "order-fulfillment-projection";
pub const EVENTS: &[&str] = &[
    event::REQUESTED,
    event::INVENTORY_RESERVED,
    event::PAYMENT_SUCCEEDED,
    event::PAYMENT_DECLINED,
    event::INVENTORY_RELEASED,
];

pub fn handle(store: &InMemoryReadModelStore, evt: &Event) -> ReadModelCommitOutcome {
    let msg = fulfillment::decode(evt);
    let step = evt
        .event_type
        .strip_prefix("fulfillment.")
        .unwrap_or(&evt.event_type)
        .to_string();

    let row = OrderFulfillmentStepView {
        order_id: msg.order_id.clone(),
        step,
        detail: msg.detail.clone(),
    };

    let mut session = store.session();
    session
        .save(&row)
        .expect("fulfillment step save should stage")
        .mark_processed(CONSUMER, &evt.id);
    session
        .commit()
        .expect("fulfillment projection should commit")
}
