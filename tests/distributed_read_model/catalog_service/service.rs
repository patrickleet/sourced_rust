use std::sync::Arc;

use sourced_rust::microsvc::Service;

use super::{handlers, CatalogRepo};

pub fn service(repo: CatalogRepo) -> Arc<Service<CatalogRepo>> {
    Arc::new(sourced_rust::register_handlers!(
        Service::new(repo),
        handlers::product_add,
        handlers::product_reprice,
    ))
}
