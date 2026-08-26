//! Bounded cell SQLite outbox claim/publish/settle scheduler.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use futures_util::future::BoxFuture;
use futures_util::{stream, StreamExt};
use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::bus::{Message, MessagePublisher};
use crate::command_dispatch::HttpCommandHost;

use super::{parse_claimed_cell_outbox, CellOutboxHint};

/// GraphQL process path a cell alarm POSTs a cell-address hint to.
pub const CELL_OUTBOX_DRAIN_PATH: &str = "/internal/outbox/drain";

const SCHEDULER_CAPACITY: usize = 1_024;
const MAX_TRACKED_CELLS: usize = 4_096;
const CLAIM_LIMIT: usize = 64;
const CLAIM_LEASE: Duration = Duration::from_secs(30);
const PUBLISH_TIMEOUT: Duration = Duration::from_secs(5);
const DRAIN_TIMEOUT: Duration = Duration::from_secs(25);

pub type CellOutboxDrainHandler =
    Arc<dyn Fn(Value) -> BoxFuture<'static, Result<(), String>> + Send + Sync>;

/// Non-blocking ingress to the one shared outbox claim/publish/settle loop.
#[derive(Clone)]
pub struct CellOutboxScheduler {
    tx: mpsc::Sender<CellOutboxHint>,
}

impl CellOutboxScheduler {
    pub fn spawn<P>(http: HttpCommandHost, publisher: P) -> Self
    where
        P: MessagePublisher + Clone + Send + Sync + 'static,
    {
        let (tx, rx) = mpsc::channel(SCHEDULER_CAPACITY);
        tokio::spawn(run_scheduler(http, publisher, rx));
        Self { tx }
    }

    /// Queue a durable cell address without waiting for HTTP or the broker.
    pub fn schedule(&self, hint: CellOutboxHint) -> Result<(), String> {
        hint.validate()?;
        self.tx.try_send(hint).map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => {
                "cell outbox scheduler is temporarily at capacity".to_string()
            }
            mpsc::error::TrySendError::Closed(_) => {
                "cell outbox scheduler is not running".to_string()
            }
        })
    }
}

async fn run_scheduler<P>(
    http: HttpCommandHost,
    publisher: P,
    mut rx: mpsc::Receiver<CellOutboxHint>,
) where
    P: MessagePublisher + Clone + Send + Sync + 'static,
{
    let worker_id = format!("cell-host-{}", uuid::Uuid::now_v7());
    let mut tracked = HashSet::new();
    let mut ticker = tokio::time::interval(Duration::from_secs(5));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    ticker.tick().await;

    loop {
        let hints = tokio::select! {
            hint = rx.recv() => {
                let Some(hint) = hint else {
                    return;
                };
                vec![hint]
            }
            _ = ticker.tick() => {
                tracked.iter().take(32).cloned().collect::<Vec<_>>()
            }
        };

        for hint in hints {
            if tracked.len() < MAX_TRACKED_CELLS || tracked.contains(&hint) {
                tracked.insert(hint.clone());
            }
            let drained = tokio::time::timeout(
                DRAIN_TIMEOUT,
                drain_one_cell(&http, &publisher, &worker_id, &hint),
            )
            .await;
            match drained {
                Ok(Ok(true)) => {
                    tracked.remove(&hint);
                }
                Ok(Ok(false)) => {}
                Ok(Err(error)) => eprintln!(
                    "cell outbox drain failed for {}/{}: {error}",
                    hint.kind, hint.id
                ),
                Err(_) => eprintln!(
                    "cell outbox drain timed out for {}/{}; its durable lease will expire",
                    hint.kind, hint.id
                ),
            }
        }
    }
}

/// Returns true only after the cell confirms that no claimable rows remain.
async fn drain_one_cell<P>(
    http: &HttpCommandHost,
    publisher: &P,
    worker_id: &str,
    hint: &CellOutboxHint,
) -> Result<bool, String>
where
    P: MessagePublisher + Clone + Send + Sync + 'static,
{
    let shard = http
        .retarget_segments(&[&hint.kind, &hint.id])
        .map_err(|error| error.to_string())?;
    let (status, body) = shard
        .post_json(
            "outbox.claim",
            json!({
                "workerId": worker_id,
                "limit": CLAIM_LIMIT,
                "leaseMs": CLAIM_LEASE.as_millis() as u64,
            }),
        )
        .await
        .map_err(|error| error.to_string())?;
    if status != 200 {
        return Err(format!("cell outbox claim returned HTTP {status}"));
    }
    let rows = parse_claimed_cell_outbox(&body)?;
    if rows.is_empty() {
        return Ok(true);
    }
    if rows.iter().any(|row| !row.is_claimed_by(worker_id)) {
        return Err("cell returned an outbox claim owned by another worker".into());
    }

    let results = stream::iter(rows.into_iter())
        .map(|row| {
            let publisher = publisher.clone();
            async move {
                let id = row.id.clone();
                let published =
                    tokio::time::timeout(PUBLISH_TIMEOUT, publisher.publish(Message::from(row)))
                        .await
                        .is_ok_and(|result| result.is_ok());
                (id, published)
            }
        })
        .buffer_unordered(8)
        .collect::<Vec<_>>()
        .await;
    let mut published = Vec::new();
    let mut failed = Vec::new();
    for (id, succeeded) in results {
        if succeeded {
            published.push(id);
        } else {
            failed.push(id);
        }
    }

    if !published.is_empty() {
        let (status, _) = shard
            .post_json(
                "outbox.complete",
                json!({ "workerId": worker_id, "ids": published }),
            )
            .await
            .map_err(|error| error.to_string())?;
        if status != 200 {
            return Err(format!("cell outbox completion returned HTTP {status}"));
        }
    }
    if !failed.is_empty() {
        let (status, _) = shard
            .post_json(
                "outbox.release",
                json!({
                    "workerId": worker_id,
                    "ids": failed,
                    "error": "broker publish failed or timed out",
                }),
            )
            .await
            .map_err(|error| error.to_string())?;
        if status != 200 {
            return Err(format!("cell outbox release returned HTTP {status}"));
        }
    }

    Ok(false)
}

/// Validate an alarm hint and submit it to the shared bounded scheduler.
pub fn accept_outbox_drain(scheduler: &CellOutboxScheduler, body: Value) -> Result<(), String> {
    let hint: CellOutboxHint = serde_json::from_value(body)
        .map_err(|error| format!("invalid cell outbox hint: {error}"))?;
    scheduler.schedule(hint)
}

/// Handler for the authenticated internal alarm route.
pub fn outbox_alarm_handler(scheduler: CellOutboxScheduler) -> CellOutboxDrainHandler {
    Arc::new(move |body: Value| {
        let scheduler = scheduler.clone();
        Box::pin(async move { accept_outbox_drain(&scheduler, body) })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn handler_rejects_payload_injection_and_unsafe_addresses() {
        let (tx, mut rx) = mpsc::channel(1);
        let scheduler = CellOutboxScheduler { tx };
        let handler = outbox_alarm_handler(scheduler);

        assert!(handler(json!({ "kind": "todo", "id": "1", "outbox": [] }))
            .await
            .is_err());
        assert!(handler(json!({ "kind": "todo", "id": "../chat" }))
            .await
            .is_err());
        handler(json!({ "kind": "todo", "id": "safe-id" }))
            .await
            .expect("safe hint");
        assert_eq!(
            rx.recv().await,
            Some(CellOutboxHint {
                kind: "todo".into(),
                id: "safe-id".into(),
            })
        );
    }

    #[tokio::test]
    async fn scheduler_ingress_is_bounded_and_nonblocking() {
        let (tx, _rx) = mpsc::channel(1);
        let scheduler = CellOutboxScheduler { tx };
        scheduler
            .schedule(CellOutboxHint::new("todo", "1").expect("hint"))
            .expect("first");
        assert!(scheduler
            .schedule(CellOutboxHint::new("todo", "2").expect("hint"))
            .is_err());
    }
}
