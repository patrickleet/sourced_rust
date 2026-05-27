//! One projection service. Subscribes to every event type the projection
//! handlers consume and dispatches each event (a message) to the owning handler.

mod handlers;

pub use handlers::board::CONSUMER as BOARD_CONSUMER;

use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use sourced_rust::bus::Subscribable;
use sourced_rust::microsvc::{self, HandlerError, Service};
use sourced_rust::{InMemoryQueue, InMemoryReadModelStore, ReadModelError, ReadModelWorkspaceExt};

use crate::read_models::{board_key, BoardView};

pub type ProjectionDependencies = InMemoryReadModelStore;

pub fn start_board_projection_service(
    queue: InMemoryQueue,
    store: InMemoryReadModelStore,
) -> microsvc::TransportHandle {
    microsvc::subscribe(
        service(store),
        queue.new_subscriber(),
        Duration::from_millis(10),
    )
}

pub fn service(store: InMemoryReadModelStore) -> Arc<Service<ProjectionDependencies>> {
    Arc::new(Service::with_read_model_store(store).handler(
        handlers::board::SPEC,
        handlers::board::guard,
        handlers::board::handle,
    ))
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
