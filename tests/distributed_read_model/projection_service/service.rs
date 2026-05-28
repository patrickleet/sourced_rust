use std::sync::Arc;

use sourced_rust::microsvc::Service;
use sourced_rust::InMemoryReadModelStore;

use super::handlers;

pub type ProjectionDependencies = InMemoryReadModelStore;

pub fn service(store: InMemoryReadModelStore) -> Arc<Service<ProjectionDependencies>> {
    Arc::new(sourced_rust::register_handlers!(
        Service::with_read_model_store(store),
        events handlers::checkout,
        events handlers::seat,
    ))
}
