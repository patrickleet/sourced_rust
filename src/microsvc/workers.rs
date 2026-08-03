//! Framework-owned outbox and consumer worker loops.
//!
//! Applications should not reimplement dialect-specific spawn loops.

use std::sync::Arc;
use std::time::Duration;

use crate::bus::{Bus, RunOptions};
use crate::microsvc::Service;
use crate::outbox_worker::{BusPublisher, OutboxDispatcher, OutboxStore};

/// Spawn the standard outbox publish loop for a bus-backed store.
pub fn spawn_outbox_publish_loop<S, B>(
    store: S,
    bus: Arc<B>,
    service_name: impl Into<String>,
    lease: Duration,
    max_attempts: u32,
) where
    S: OutboxStore + 'static,
    B: Bus + Send + Sync + 'static,
{
    let service_name = service_name.into();
    tokio::spawn(async move {
        let dispatcher = OutboxDispatcher::new(
            store,
            BusPublisher::new(bus),
            format!("outbox:{}", std::process::id()),
            lease,
            max_attempts,
        )
        .with_service(service_name);
        loop {
            match dispatcher.dispatch_batch(32).await {
                Ok(o) if o.published > 0 || o.claimed > 0 => {}
                Ok(_) => tokio::time::sleep(Duration::from_millis(25)).await,
                Err(e) => {
                    eprintln!("outbox: {e}");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    });
}

/// Spawn a service consumer loop that re-runs the bus handler continuously.
pub fn spawn_service_consumer_loop<F>(build_service: F)
where
    F: Fn() -> Service + Send + Sync + 'static,
{
    tokio::spawn(async move {
        loop {
            let service = build_service();
            match service.run(RunOptions::idempotent()).await {
                Ok(()) => tokio::time::sleep(Duration::from_millis(25)).await,
                Err(e) => {
                    eprintln!("consumer: {e}");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    });
}
