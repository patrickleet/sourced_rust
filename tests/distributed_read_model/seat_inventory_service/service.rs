use std::sync::Arc;

use distributed::microsvc::{Routes, Service};

use super::{handlers, SeatRepo};

pub fn service(repo: SeatRepo) -> Arc<Service> {
    Arc::new(Service::new().routes(distributed::routes!(
        Routes::new().with_repo(repo),
        command handlers::add,
        event handlers::reserve_started_checkout_seat,
    )))
}
