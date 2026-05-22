use std::sync::mpsc::{self, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use sourced_rust::bus::{Bus, Event};
use sourced_rust::{HashMapRepository, InMemoryQueue, ReadModelsExt};

use crate::models::aggregates::account::AccountSnapshot;
use crate::models::readmodels::account_summary::AccountSummary;

pub struct ReadModelServiceHandle {
    stop_tx: mpsc::Sender<()>,
    handle: thread::JoinHandle<()>,
}

impl ReadModelServiceHandle {
    pub fn stop(self) {
        let _ = self.stop_tx.send(());
        self.handle
            .join()
            .expect("read model service should stop cleanly");
    }
}

pub fn start_account_summary_service(
    queue: InMemoryQueue,
    store: HashMapRepository,
) -> ReadModelServiceHandle {
    let (stop_tx, stop_rx) = mpsc::channel();
    let (ready_tx, ready_rx) = mpsc::channel();

    let handle = thread::spawn(move || {
        let bus = Bus::from_queue(queue);
        let events = bus.subscribe(&["AccountOpened", "MoneyDeposited"]);
        ready_tx
            .send(())
            .expect("read model service should signal readiness");

        loop {
            match stop_rx.try_recv() {
                Ok(()) | Err(TryRecvError::Disconnected) => break,
                Err(TryRecvError::Empty) => {}
            }

            match events.recv(10) {
                Ok(Some(event)) => {
                    project_account_summary(&store, &event);
                    events
                        .ack(&event.id)
                        .expect("read model service should ack projected events");
                }
                Ok(None) => {}
                Err(err) => panic!("read model service failed to receive event: {err}"),
            }
        }
    });

    ready_rx
        .recv_timeout(Duration::from_secs(3))
        .expect("read model service should subscribe before accepting writes");

    ReadModelServiceHandle { stop_tx, handle }
}

fn load_summary(store: &HashMapRepository, account_id: &str) -> AccountSummary {
    store
        .read_models::<AccountSummary>()
        .get(account_id)
        .expect("read model load should succeed")
        .map(|view| view.data)
        .unwrap_or_else(|| AccountSummary::empty(account_id))
}

fn project_account_summary(store: &HashMapRepository, event: &Event) {
    let snapshot: AccountSnapshot = event.decode().expect("account snapshot should decode");
    let mut summary = load_summary(store, &snapshot.id);

    if summary.has_projected(&event.id) {
        return;
    }

    match event.event_type.as_str() {
        "AccountOpened" => {
            summary.owner = Some(snapshot.owner);
        }
        "MoneyDeposited" => {
            summary.owner = Some(snapshot.owner);
            summary.balance_cents = snapshot.balance_cents;
            summary.deposit_count += 1;
        }
        other => panic!("unexpected account event: {other}"),
    }

    summary.mark_projected(&event.id);
    store
        .read_models::<AccountSummary>()
        .upsert(&summary)
        .expect("account summary projection should persist");
}

pub fn wait_for_summary(
    store: &HashMapRepository,
    account_id: &str,
    ready: impl Fn(&AccountSummary) -> bool,
) -> AccountSummary {
    let deadline = Instant::now() + Duration::from_secs(10);

    loop {
        if let Some(summary) = store
            .read_models::<AccountSummary>()
            .get(account_id)
            .expect("read model load should succeed")
            .map(|view| view.data)
        {
            if ready(&summary) {
                return summary;
            }
        }

        assert!(
            Instant::now() < deadline,
            "timed out waiting for account summary projection"
        );
        thread::sleep(Duration::from_millis(10));
    }
}
