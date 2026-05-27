use std::sync::Arc;

use sourced_rust::microsvc::Service;
use sourced_rust::InMemoryReadModelStore;

use super::handlers;

pub type ProjectionDependencies = InMemoryReadModelStore;

pub fn service(store: InMemoryReadModelStore) -> Arc<Service<ProjectionDependencies>> {
    Arc::new(
        Service::with_read_model_store(store)
            .handler(
                handlers::checkout::SPEC,
                handlers::checkout::guard,
                handlers::checkout::handle,
            )
            .handler(
                handlers::seat::SPEC,
                handlers::seat::guard,
                handlers::seat::handle,
            ),
    )
}

pub fn projects(event_type: &str) -> bool {
    handlers::checkout::EVENTS.contains(&event_type) || handlers::seat::EVENTS.contains(&event_type)
}
