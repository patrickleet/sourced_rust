//! One projection service. Subscribes to every event type the projection
//! handlers consume and dispatches each event (a message) to the owning handler.

mod handlers;

pub use handlers::board::CONSUMER as BOARD_CONSUMER;

use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;
use sourced_rust::bus::{Event, PublishError, Subscribable, Subscriber};
use sourced_rust::microsvc::{self, Context, HandlerError, Service};
use sourced_rust::{InMemoryQueue, InMemoryReadModelStore, ReadModelError, ReadModelWorkspaceExt};

use crate::read_models::{board_key, BoardView};

pub type ProjectionDependencies = InMemoryReadModelStore;

type ProjectionGuard = fn(&Context<ProjectionDependencies>) -> bool;
type ProjectionHandler = fn(&Context<ProjectionDependencies>) -> Result<Value, HandlerError>;

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
                    let payload = serde_json::to_vec(&handlers::ProjectionMessage::from(&event))
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

pub fn start_board_projection_service(
    queue: InMemoryQueue,
    store: InMemoryReadModelStore,
) -> microsvc::TransportHandle {
    microsvc::subscribe(
        service(store),
        subscriber(queue.new_subscriber()),
        Duration::from_millis(10),
    )
}

pub fn service(store: InMemoryReadModelStore) -> Arc<Service<ProjectionDependencies>> {
    let service = register_handler_events(
        Service::with_read_model_store(store),
        handlers::board::EVENTS,
        handlers::board::guard,
        handlers::board::handle,
    );
    Arc::new(service)
}

pub fn subscriber<S>(subscriber: S) -> ProjectionSubscriber<S>
where
    S: Subscriber,
{
    ProjectionSubscriber { inner: subscriber }
}

fn register_handler_events(
    mut service: Service<ProjectionDependencies>,
    events: &[&str],
    guard: ProjectionGuard,
    handle: ProjectionHandler,
) -> Service<ProjectionDependencies> {
    for event in events {
        service = service.command_guarded(event, guard, handle);
    }
    service
}

fn read_model_error(err: ReadModelError) -> HandlerError {
    HandlerError::Repository(err.into())
}

pub fn wait_for_board(
    store: &InMemoryReadModelStore,
    board_id: &str,
    ready: impl Fn(&BoardView) -> bool,
) -> BoardView {
    let deadline = Instant::now() + Duration::from_secs(10);

    loop {
        let mut session = store.workspace();
        if let Some(board) = session
            .load::<BoardView>(board_key(board_id))
            .include("cards")
            .one()
            .expect("board load should succeed")
            .map(|view| view.data)
        {
            if ready(&board) {
                return board;
            }
        }

        assert!(
            Instant::now() < deadline,
            "timed out waiting for board projection"
        );
        thread::sleep(Duration::from_millis(10));
    }
}
