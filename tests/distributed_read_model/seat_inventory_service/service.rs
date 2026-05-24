use std::sync::Arc;

use sourced_rust::microsvc::Service;

use super::{handlers, SeatRepo};

pub fn service(repo: SeatRepo) -> Arc<Service<SeatRepo>> {
    let service = sourced_rust::register_handlers!(Service::new(repo), handlers::add);
    Arc::new(service.command_guarded(
        handlers::reserve_started_checkout_seat::EVENT,
        handlers::reserve_started_checkout_seat::guard,
        handlers::reserve_started_checkout_seat::handle,
    ))
}
