//! Projection service. It subscribes to every event type the projection handlers
//! consume and dispatches each event through `microsvc::Service`. Handlers mark
//! messages processed in the same commit for idempotency.

mod handlers;
mod service;

pub use handlers::fulfillment::CONSUMER as FULFILLMENT_CONSUMER;
pub use handlers::order::CONSUMER as ORDER_CONSUMER;
pub use handlers::product::CONSUMER as CATALOG_CONSUMER;
pub use service::{service, subscriber};
