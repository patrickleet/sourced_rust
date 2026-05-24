use std::sync::Arc;

use sourced_rust::microsvc::Service;

use super::{handlers, PaymentRepo};

pub fn model_service(repo: PaymentRepo) -> Arc<Service<PaymentRepo>> {
    Arc::new(sourced_rust::register_handlers!(
        Service::new(repo),
        handlers::charge,
    ))
}
