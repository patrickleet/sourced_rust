use std::sync::Arc;

use sourced_rust::microsvc::Service;

use super::{handlers, SagaRepo};

pub fn model_service(repo: SagaRepo) -> Arc<Service<SagaRepo>> {
    Arc::new(sourced_rust::register_handlers!(
        Service::new(repo),
        handlers::on_requested,
        handlers::on_inventory_reserved,
        handlers::on_payment_succeeded,
        handlers::on_payment_declined,
        handlers::on_inventory_released,
    ))
}
