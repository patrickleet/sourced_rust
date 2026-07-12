//! Commit-path subscription invalidation (live query refresh).
//!
//! Each subscription field:
//! 1. Executes the query once and yields the initial result.
//! 2. Listens on [`ChangeHub`] (fed by `change_stream` / repo broadcast).
//! 3. On dirty tables intersecting the plan footprint, debounces, re-executes,
//!    and yields only when the response hash changes (hash-gated push).

use std::collections::BTreeSet;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use async_graphql::Value;
use futures_util::Stream;
use tokio::sync::{broadcast, mpsc};

use crate::microsvc::Session;
use crate::read_model::ReadModelChange;

use super::compile::{self, RootKind, SelectionNode, SqlPlan};
use super::engine::{execute_plan, EngineInner};

/// Fan-out hub for read-model change notifications.
#[derive(Clone, Debug)]
pub struct ChangeHub {
    tx: broadcast::Sender<ReadModelChange>,
}

impl ChangeHub {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(256);
        Self { tx }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ReadModelChange> {
        self.tx.subscribe()
    }

    pub fn publish(&self, change: ReadModelChange) {
        if change.is_empty() {
            // Empty set is reserved as the all-dirty lag signal; allow it only
            // when explicitly published for that purpose (forwarder).
        }
        let _ = self.tx.send(change);
    }

    /// Forward an external receiver into this hub until the source closes.
    pub fn spawn_forward_from(&self, mut rx: broadcast::Receiver<ReadModelChange>) {
        let tx = self.tx.clone();
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(change) => {
                        let _ = tx.send(change);
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        // Empty tables = all-dirty for subscribers.
                        let _ = tx.send(ReadModelChange {
                            tables: BTreeSet::new(),
                        });
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }
}

impl Default for ChangeHub {
    fn default() -> Self {
        Self::new()
    }
}

// Yield `Value` (not FieldValue<'static>) so HRTB `Into<FieldValue<'a>>` holds for any 'a.
type LiveItem = Result<Value, async_graphql::Error>;

/// Stream of GraphQL field values for one live subscription.
pub struct LiveQueryStream {
    rx: mpsc::Receiver<LiveItem>,
}

impl Stream for LiveQueryStream {
    type Item = LiveItem;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.rx.poll_recv(cx)
    }
}

/// Build a live-query stream for a subscription root field.
pub(crate) async fn live_query_stream(
    inner: Arc<EngineInner>,
    session: Session,
    role: String,
    model: String,
    selection: SelectionNode,
) -> Result<LiveQueryStream, String> {
    let plan: SqlPlan =
        compile::compile_root(&inner, &session, &role, &model, RootKind::List, &selection)?;
    let footprint = footprint_from_tables(&plan.tables_touched);
    let mut change_rx = inner.change_hub.subscribe();
    let (tx, rx) = mpsc::channel::<LiveItem>(8);
    let debounce = Duration::from_millis(100);

    tokio::spawn(async move {
        // 1) Initial execution + yield
        let mut last_hash = match execute_list(&inner, &session, &role, &model, &selection).await {
            Ok(value) => {
                let h = response_hash(&value);
                if tx.send(Ok(value)).await.is_err() {
                    return;
                }
                Some(h)
            }
            Err(e) => {
                let _ = tx.send(Err(async_graphql::Error::new(e))).await;
                return;
            }
        };

        // 2) Change loop: dirty → debounce → re-exec → hash-gate → yield
        loop {
            let change = match change_rx.recv().await {
                Ok(c) => c,
                Err(broadcast::error::RecvError::Lagged(_)) => ReadModelChange {
                    tables: BTreeSet::new(),
                },
                Err(broadcast::error::RecvError::Closed) => break,
            };

            if !footprint_hits(&footprint, &change) {
                continue;
            }

            // Debounce / coalesce
            tokio::time::sleep(debounce).await;
            loop {
                match change_rx.try_recv() {
                    Ok(more) => {
                        // Keep waiting only if still relevant; either way we re-exec once.
                        let _ = more;
                    }
                    Err(broadcast::error::TryRecvError::Empty) => break,
                    Err(broadcast::error::TryRecvError::Lagged(_)) => break,
                    Err(broadcast::error::TryRecvError::Closed) => return,
                }
            }

            match execute_list(&inner, &session, &role, &model, &selection).await {
                Ok(value) => {
                    let h = response_hash(&value);
                    if last_hash == Some(h) {
                        continue; // hash gate: no push on no-change
                    }
                    last_hash = Some(h);
                    if tx.send(Ok(value)).await.is_err() {
                        return;
                    }
                }
                Err(e) => {
                    if tx.send(Err(async_graphql::Error::new(e))).await.is_err() {
                        return;
                    }
                }
            }
        }
    });

    Ok(LiveQueryStream { rx })
}

async fn execute_list(
    inner: &EngineInner,
    session: &Session,
    role: &str,
    model: &str,
    selection: &SelectionNode,
) -> Result<Value, String> {
    let plan = compile::compile_root(inner, session, role, model, RootKind::List, selection)?;
    execute_plan(inner, &plan).await
}

fn footprint_hits(footprint: &BTreeSet<String>, change: &ReadModelChange) -> bool {
    // Empty tables = all-dirty (lag signal from forwarder).
    if change.tables.is_empty() {
        return true;
    }
    change.tables.iter().any(|t| footprint.contains(t))
}

/// Compute the table footprint of a compiled plan (for dirty matching).
pub fn footprint_from_tables(tables: &[String]) -> BTreeSet<String> {
    tables.iter().cloned().collect()
}

/// Hash a GraphQL JSON payload for hash-gated push (no push on no-change).
pub fn response_hash(value: &Value) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    if let Ok(json) = serde_json::to_string(value) {
        json.hash(&mut h);
    } else {
        format!("{value:?}").hash(&mut h);
    }
    h.finish()
}

/// Forward an external change receiver into the engine hub.
pub fn spawn_change_forwarder(hub: ChangeHub, rx: broadcast::Receiver<ReadModelChange>) {
    hub.spawn_forward_from(rx);
}
