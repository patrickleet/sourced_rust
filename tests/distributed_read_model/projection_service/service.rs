use std::sync::Arc;

use serde_json::Value;
use sourced_rust::bus::{Event, PublishError, Subscriber};
use sourced_rust::microsvc::{Context, HandlerError, Service};
use sourced_rust::InMemoryReadModelStore;

use super::handlers::{self, ProjectionMessage};

type ProjectionGuard = fn(&Context<InMemoryReadModelStore>) -> bool;
type ProjectionHandler = fn(&Context<InMemoryReadModelStore>) -> Result<Value, HandlerError>;

pub struct ProjectionSubscriber<S> {
    inner: S,
}

impl<S> Subscriber for ProjectionSubscriber<S>
where
    S: Subscriber,
{
    fn poll(&self, timeout_ms: u64) -> Result<Option<Event>, PublishError> {
        self.inner.poll(timeout_ms).and_then(|event| {
            event
                .map(|event| {
                    let payload = serde_json::to_vec(&ProjectionMessage::from(&event))
                        .map_err(|err| PublishError::SerializationFailed(err.to_string()))?;
                    let mut wrapped = Event::new(event.id, event.event_type, payload);
                    wrapped.metadata = event.metadata;
                    Ok(wrapped)
                })
                .transpose()
        })
    }

    fn ack(&self, event_id: &str) -> Result<(), PublishError> {
        self.inner.ack(event_id)
    }

    fn nack(&self, event_id: &str, reason: &str) -> Result<(), PublishError> {
        self.inner.nack(event_id, reason)
    }
}

pub fn service(store: InMemoryReadModelStore) -> Arc<Service<InMemoryReadModelStore>> {
    let service = Service::new(store);
    let service = register_handler_events(
        service,
        handlers::checkout::EVENTS,
        handlers::checkout::guard,
        handlers::checkout::handle,
    );
    let service = register_handler_events(
        service,
        handlers::seat::EVENTS,
        handlers::seat::guard,
        handlers::seat::handle,
    );

    Arc::new(service)
}

pub fn subscriber<S>(subscriber: S) -> ProjectionSubscriber<S>
where
    S: Subscriber,
{
    ProjectionSubscriber { inner: subscriber }
}

pub fn projects(event_type: &str) -> bool {
    handlers::checkout::EVENTS.contains(&event_type) || handlers::seat::EVENTS.contains(&event_type)
}

fn register_handler_events(
    mut service: Service<InMemoryReadModelStore>,
    events: &[&str],
    guard: ProjectionGuard,
    handle: ProjectionHandler,
) -> Service<InMemoryReadModelStore> {
    for event in events {
        service = service.command_guarded(event, guard, handle);
    }
    service
}
