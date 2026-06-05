use std::sync::Arc;

use distributed::microsvc::Service;

use super::{handlers, SeatRepo};

pub fn service(repo: SeatRepo) -> Arc<Service<SeatRepo>> {
    Arc::new(distributed::register_handlers!(
        Service::new().with_repo(repo),
        command handlers::add,
        event handlers::reserve_started_checkout_seat,
    ))
}
