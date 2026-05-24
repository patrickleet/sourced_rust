//! One projection service. Subscribes to every event type the projection
//! handlers consume and dispatches each event (a message) to the owning handler.

mod handlers;

pub use handlers::board::CONSUMER as BOARD_CONSUMER;

use std::sync::mpsc::{self, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use sourced_rust::bus::Bus;
use sourced_rust::{InMemoryQueue, InMemoryReadModelStore, ReadModelUnitOfWorkExt};

use crate::read_models::{board_key, BoardView};

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

pub fn start_board_projection_service(
    queue: InMemoryQueue,
    store: InMemoryReadModelStore,
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
                    handlers::project(&store, &event);
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

pub fn wait_for_board(
    store: &InMemoryReadModelStore,
    board_id: &str,
    ready: impl Fn(&BoardView) -> bool,
) -> BoardView {
    let deadline = Instant::now() + Duration::from_secs(10);

    loop {
        let mut session = store.session();
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
