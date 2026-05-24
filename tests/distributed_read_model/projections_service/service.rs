use std::collections::HashMap;
use std::sync::mpsc::{self, TryRecvError};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use serde_json::Value;
use sourced_rust::bus::{Bus, Event};
use sourced_rust::microsvc::{Context, HandlerError, Service, Session};
use sourced_rust::{InMemoryQueue, InMemoryReadModelStore};

use super::handlers::{self, ProjectionMessage};

type ProjectionGuard = fn(&Context<InMemoryReadModelStore>) -> bool;
type ProjectionHandler = fn(&Context<InMemoryReadModelStore>) -> Result<Value, HandlerError>;

pub struct ProjectionServiceHandle {
    stop_tx: mpsc::Sender<()>,
    handle: thread::JoinHandle<()>,
}

impl ProjectionServiceHandle {
    pub fn stop(self) {
        let _ = self.stop_tx.send(());
        self.handle
            .join()
            .expect("projection service should stop cleanly");
    }
}

pub fn service(store: InMemoryReadModelStore) -> Arc<Service<InMemoryReadModelStore>> {
    let service = Service::new(store);
    let service = register_handler_events(
        service,
        handlers::product::EVENTS,
        handlers::product::guard,
        handlers::product::handle,
    );
    let service = register_handler_events(
        service,
        handlers::order::EVENTS,
        handlers::order::guard,
        handlers::order::handle,
    );
    let service = register_handler_events(
        service,
        handlers::fulfillment::EVENTS,
        handlers::fulfillment::guard,
        handlers::fulfillment::handle,
    );

    Arc::new(service)
}

pub fn start_projection_service(
    queue: InMemoryQueue,
    service: Arc<Service<InMemoryReadModelStore>>,
) -> ProjectionServiceHandle {
    let (stop_tx, stop_rx) = mpsc::channel();
    let (ready_tx, ready_rx) = mpsc::channel();

    let handle = thread::spawn(move || {
        let bus = Bus::from_queue(queue);
        let event_types = handlers::event_types();
        let events = bus.subscribe(&event_types);
        ready_tx
            .send(())
            .expect("projection service should signal readiness");

        loop {
            match stop_rx.try_recv() {
                Ok(()) | Err(TryRecvError::Disconnected) => break,
                Err(TryRecvError::Empty) => {}
            }

            match events.recv(10) {
                Ok(Some(event)) => {
                    let input = serde_json::to_value(ProjectionMessage::from(&event))
                        .expect("projection event envelope should encode");
                    service
                        .dispatch(&event.event_type, input, session_from_event(&event))
                        .unwrap_or_else(|err| {
                            panic!(
                                "projection service failed to dispatch {}: {err}",
                                event.event_type
                            )
                        });
                    events
                        .ack(&event.id)
                        .expect("projection service should ack projected events");
                }
                Ok(None) => {}
                Err(err) => panic!("projection service failed to receive event: {err}"),
            }
        }
    });

    ready_rx
        .recv_timeout(Duration::from_secs(3))
        .expect("projection service should subscribe before accepting writes");

    ProjectionServiceHandle { stop_tx, handle }
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

fn session_from_event(event: &Event) -> Session {
    let Some(metadata) = &event.metadata else {
        return Session::new();
    };

    Session::from_map(
        metadata
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<HashMap<_, _>>(),
    )
}
