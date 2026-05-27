use std::sync::Arc;

use sourced_rust::microsvc::Service;

use super::{handlers, SeatRepo};

pub fn service(repo: SeatRepo) -> Arc<Service<SeatRepo>> {
    Arc::new(sourced_rust::register_handlers!(
        Service::with_repo(repo),
        handlers::add,
        event handlers::reserve_started_checkout_seat,
    ))
}
