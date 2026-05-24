use std::sync::Arc;

use sourced_rust::microsvc::Service;

use super::{handlers, SagaRepo};

pub fn service(repo: SagaRepo) -> Arc<Service<SagaRepo>> {
    Arc::new(sourced_rust::register_handlers!(
        Service::new(repo),
        handlers::start,
        handlers::record_inventory_reserved,
        handlers::record_payment_succeeded,
        handlers::record_payment_declined,
        handlers::record_inventory_released,
    ))
}
