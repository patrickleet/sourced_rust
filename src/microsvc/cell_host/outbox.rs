//! Cell SQLite outbox drain through the process [`MessagePublisher`].
//!
//! Same for every aggregate: publish, fire-and-forget `outbox.complete`, and
//! re-read still-Pending rows. The cell is the durable store — not a second SQL.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::future::BoxFuture;
use serde_json::{json, Value};

use crate::bus::{Message, MessagePublisher};
use crate::command_dispatch::HttpCommandHost;
use crate::microsvc::{CausalDispatchResult, Session};
use crate::OutboxMessage;

/// GraphQL process path a cell alarm POSTs pending rows to.
pub const CELL_OUTBOX_DRAIN_PATH: &str = "/internal/outbox/drain";

pub type CellOutboxDrainHandler =
    Arc<dyn Fn(Value) -> BoxFuture<'static, ()> + Send + Sync>;

/// Mark cell SQLite rows Published after bus `Ok`. Do not await before the
/// mutation returns.
pub fn complete_cell_outbox_later(http: &HttpCommandHost, ids: Vec<String>) {
    if ids.is_empty() {
        return;
    }
    let http = http.clone();
    tokio::spawn(async move {
        let _ = http
            .post_json("outbox.complete", json!({ "ids": ids }))
            .await;
    });
}

/// Publish pending cell outbox through the process bus. On `Ok`, spawn
/// `outbox.complete` and return. Publish `Err` retries in-process.
pub async fn drain_cell_outbox<P>(http: &HttpCommandHost, publisher: &P, rows: &[OutboxMessage])
where
    P: MessagePublisher + Clone + Send + Sync + 'static,
{
    let mut published = Vec::new();
    for row in rows {
        if publisher
            .publish(Message::from(row.clone()))
            .await
            .is_ok()
        {
            published.push(row.id.clone());
            continue;
        }
        let publisher = publisher.clone();
        let complete = http.clone();
        let row = row.clone();
        tokio::spawn(async move {
            for backoff_ms in [50_u64, 100, 200, 400, 800, 1600, 3200] {
                tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                if publisher.publish(Message::from(row.clone())).await.is_ok() {
                    complete_cell_outbox_later(&complete, vec![row.id.clone()]);
                    return;
                }
            }
            eprintln!(
                "cell outbox: bus publish still failing for {}; cell SQLite still has the row",
                row.id
            );
        });
    }
    complete_cell_outbox_later(http, published);
}

/// Extra drainer: every 5s, `POST {celld}/{kind}/{id}/outbox.drain` for cells
/// this process has seen and re-publish still-Pending rows.
pub fn spawn_cell_outbox_drain_loop<P>(
    http: HttpCommandHost,
    publisher: P,
    pending: Arc<Mutex<HashSet<(String, String)>>>,
) where
    P: MessagePublisher + Clone + Send + Sync + 'static,
{
    let celld_url = http.base().to_string();
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(5));
        ticker.tick().await;
        loop {
            ticker.tick().await;
            let cells: Vec<(String, String)> = match pending.lock() {
                Ok(guard) => guard.iter().cloned().collect(),
                Err(_) => continue,
            };
            for (kind, id) in cells {
                let shard = http.retarget(format!("{celld_url}/{kind}/{id}"));
                let Ok((_, body)) = shard
                    .post_wait_path(
                        "outbox.drain",
                        "drain",
                        json!({}),
                        &Session::new(),
                    )
                    .await
                else {
                    continue;
                };
                let rows = CausalDispatchResult::outbox_from_wait_path(&body);
                if rows.is_empty() {
                    if let Ok(mut guard) = pending.lock() {
                        guard.remove(&(kind, id));
                    }
                    continue;
                }
                drain_cell_outbox(&shard, &publisher, &rows).await;
            }
        }
    });
}

/// Cell alarm body `{ kind, id, outbox }` → bus publish + complete.
pub async fn accept_outbox_drain<P>(
    publisher: &P,
    http: &HttpCommandHost,
    celld_url: &str,
    body: &Value,
) where
    P: MessagePublisher + Clone + Send + Sync + 'static,
{
    let Some(kind) = body
        .get("kind")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return;
    };
    let Some(id) = body
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return;
    };
    let rows = CausalDispatchResult::outbox_from_wait_path(body);
    let shard = http.retarget(format!("{celld_url}/{kind}/{id}"));
    drain_cell_outbox(&shard, publisher, &rows).await;
}

/// Handler for `POST /internal/outbox/drain` (cell alarm → this process).
pub fn outbox_alarm_handler<P>(publisher: P, celld_url: impl Into<String>) -> CellOutboxDrainHandler
where
    P: MessagePublisher + Clone + Send + Sync + 'static,
{
    let celld_url = celld_url.into().trim_end_matches('/').to_string();
    let http = HttpCommandHost::new(&celld_url);
    Arc::new(move |body: Value| {
        let http = http.clone();
        let publisher = publisher.clone();
        let celld_url = celld_url.clone();
        Box::pin(async move {
            accept_outbox_drain(&publisher, &http, &celld_url, &body).await;
        })
    })
}
