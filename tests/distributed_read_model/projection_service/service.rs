use std::sync::Arc;

use distributed::microsvc::Service;
use distributed::InMemoryReadModelStore;

use super::handlers;

pub type ProjectionDependencies = InMemoryReadModelStore;

pub fn service(store: InMemoryReadModelStore) -> Arc<Service<ProjectionDependencies>> {
    Arc::new(distributed::register_handlers!(
        Service::with_read_model_store(store),
        events handlers::checkout,
        events handlers::seat,
    ))
}
