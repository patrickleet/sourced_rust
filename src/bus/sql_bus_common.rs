//! Shared machinery for the SQL-backed buses (`PostgresBus`, `SqliteBus`).
//!
//! Postgres and SQLite implement the same bus model — a claim-lease work queue
//! (`bus_queue`) for point-to-point commands, and an append-only log plus a
//! per-consumer offset table (`bus_log` + `bus_offset`) for fan-out events.
//! They differ only in SQL dialect (placeholder style, database clock,
//! name-list binding, claim-token minting) and pool type. Mirroring
//! [`lock::sqlx_common`](crate::lock), everything else — the builders, the
//! [`Bus`]/[`BusConsumer`] impls, the queue/log sources, row decoding, and
//! settlement — lives here, generic over a [`SqlBusDialect`].
//!
//! ## Settlement model
//!
//! - **Queue (`listen`):** a claim mints a fresh `claim_token` under a lease;
//!   every settlement is fenced by `seq AND claim_token`, so a worker whose
//!   lease expired (and whose row was reclaimed under a new token) cannot
//!   settle the newer claim. `ack`/`dead_letter`/`park` delete the row; `nack`
//!   releases it for redelivery.
//! - **Log (`subscribe`):** `ack` advances the consumer's offset to the entry's
//!   `seq` (the effectively-once point); `nack` leaves the offset unmoved so the
//!   entry is re-read; `dead_letter`/`park` advance past poison entries.
//!
//! ## Corruption handling
//!
//! A row that fails to decode a required column (`name`, `kind`, `payload`,
//! `content_type`, `metadata`) is permanent corruption. Returning a placeholder
//! message would route it to the runner's ack-and-ignore path, silently deleting
//! the row (queue) or advancing the offset past it (log) with no trace. Instead
//! the decode failure is surfaced through [`ReceivedMessage::decode_error`] so
//! the runner settles the claim through the configured failure policy
//! (dead-letter by default), like any other permanent failure.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use sqlx::{ColumnIndex, Decode, Row, Type};

use crate::sqlx_repo::is_sqlx_transient;

use super::source::{MessageSource, ReceivedMessage};
use super::{
    run_source, Bus, BusConsumer, BusTopologyConfig, MessageRouter, RunOptions, TransportError,
    TransportErrorKind,
};
use super::{Message, MessageKind};

pub(crate) const DEFAULT_LEASE: Duration = Duration::from_secs(30);

/// Classify a database error transient vs permanent (via [`is_sqlx_transient`])
/// and wrap it as a `"{backend} bus {context}"` transport error. Deterministic
/// failures reach the failure policy instead of redelivering forever.
pub(crate) fn db_err(backend: &str, context: &str, err: sqlx::Error) -> TransportError {
    let kind = if is_sqlx_transient(&err) {
        TransportErrorKind::Retryable
    } else {
        TransportErrorKind::Permanent
    };
    TransportError::new(kind, format!("{backend} bus {context}: {err}")).with_source(err)
}

pub(crate) fn metadata_json(message: &Message) -> String {
    serde_json::to_string(&message.metadata).unwrap_or_else(|_| "[]".into())
}

fn corrupt_row(backend: &str, message: impl Into<String>) -> TransportError {
    TransportError::permanent(format!("{backend} bus corrupt row: {}", message.into()))
}

fn decode_err(backend: &str, column: &str, err: sqlx::Error) -> TransportError {
    corrupt_row(
        backend,
        format!("required column '{column}' failed to decode: {err}"),
    )
}

fn parse_message_kind(backend: &str, value: &str) -> Result<MessageKind, TransportError> {
    match value {
        "command" => Ok(MessageKind::Command),
        "event" => Ok(MessageKind::Event),
        _ => Err(corrupt_row(
            backend,
            format!("required column 'kind' has unsupported value {value:?}"),
        )),
    }
}

fn parse_metadata(backend: &str, value: &str) -> Result<Vec<(String, String)>, TransportError> {
    serde_json::from_str(value).map_err(|err| {
        corrupt_row(
            backend,
            format!("required column 'metadata' failed to parse as JSON metadata: {err}"),
        )
    })
}

/// Reconstruct a [`Message`] from a claimed `bus_queue`/`bus_log` row.
///
/// Required-column decode/parsing failures are permanent corruption. The row has
/// already been selected or claimed, so callers surface the error through
/// [`ReceivedMessage::decode_error`] and let the runner settle it through the
/// configured failure policy.
fn message_from_row<R>(backend: &str, row: &R) -> Result<Message, TransportError>
where
    R: Row,
    for<'a> &'a str: ColumnIndex<R>,
    for<'r> String: Decode<'r, R::Database> + Type<R::Database>,
    for<'r> Vec<u8>: Decode<'r, R::Database> + Type<R::Database>,
{
    let name: String = row
        .try_get("name")
        .map_err(|err| decode_err(backend, "name", err))?;
    let kind: String = row
        .try_get("kind")
        .map_err(|err| decode_err(backend, "kind", err))?;
    let payload: Vec<u8> = row
        .try_get("payload")
        .map_err(|err| decode_err(backend, "payload", err))?;
    let content_type: String = row
        .try_get("content_type")
        .map_err(|err| decode_err(backend, "content_type", err))?;
    if content_type.is_empty() {
        return Err(corrupt_row(
            backend,
            "required column 'content_type' is empty",
        ));
    }
    let metadata_json: String = row
        .try_get("metadata")
        .map_err(|err| decode_err(backend, "metadata", err))?;
    let metadata = parse_metadata(backend, &metadata_json)?;

    Ok(Message {
        id: row
            .try_get::<Option<String>, _>("message_id")
            .unwrap_or(None),
        name,
        kind: parse_message_kind(backend, &kind)?,
        payload,
        content_type,
        metadata,
    })
}

/// A decoded `bus_queue`/`bus_log` row: its `seq` plus either the message or the
/// decode failure (with an empty placeholder message the runner never
/// dispatches — it sees `decode_error()` first).
pub struct ReceivedRow {
    seq: i64,
    message: Message,
    decode_error: Option<TransportError>,
}

impl ReceivedRow {
    /// Decode a row, capturing a required-column failure as `decode_error`
    /// instead of losing the claim. The placeholder message's name is
    /// deliberately empty — the decode error carries the diagnostic.
    pub(crate) fn from_row<R>(backend: &str, row: &R) -> Self
    where
        R: Row,
        for<'a> &'a str: ColumnIndex<R>,
        for<'r> String: Decode<'r, R::Database> + Type<R::Database>,
        for<'r> Vec<u8>: Decode<'r, R::Database> + Type<R::Database>,
        for<'r> i64: Decode<'r, R::Database> + Type<R::Database>,
    {
        let seq = row.try_get("seq").unwrap_or_default();
        let (message, decode_error) = match message_from_row(backend, row) {
            Ok(message) => (message, None),
            Err(error) => (
                Message::new("", MessageKind::Event, Vec::new()),
                Some(error),
            ),
        };
        Self {
            seq,
            message,
            decode_error,
        }
    }
}

/// A claimed `bus_queue` row: the decoded row plus the claim token that fences
/// its settlement.
pub struct ClaimedRow {
    pub(crate) row: ReceivedRow,
    pub(crate) claim_token: String,
}

/// The dialect-specific SQL a [`SqlBus`] runs. Implemented by the per-backend
/// dialects; the shared bus, sources, and settle handles drive it.
pub trait SqlBusDialect: Clone + Send + Sync + 'static {
    /// Backend name used in error messages and consumer-group resolution.
    const BACKEND: &'static str;
    /// Idempotent DDL for `bus_queue`/`bus_log`/`bus_offset`, `;`-separated.
    const SCHEMA: &'static str;

    /// Execute one DDL statement from [`SCHEMA`](Self::SCHEMA) (hence
    /// `'static`: statements are always slices of the schema const).
    fn execute_ddl(
        &self,
        statement: &'static str,
    ) -> impl Future<Output = Result<(), TransportError>> + Send;

    /// Insert a command into `bus_queue`.
    fn insert_queue(
        &self,
        message: &Message,
    ) -> impl Future<Output = Result<(), TransportError>> + Send;

    /// Append an event to `bus_log`.
    fn insert_log(
        &self,
        message: &Message,
    ) -> impl Future<Output = Result<(), TransportError>> + Send;

    /// Atomically claim the next available `bus_queue` row whose `name` matches
    /// one of `names` **or is NULL** (un-routable corruption that must be
    /// claimed so the failure policy can settle it, not left as a poison row
    /// blocking the drain), minting a fresh claim token under `lease_secs`.
    fn claim(
        &self,
        names: &[String],
        lease_secs: f64,
    ) -> impl Future<Output = Result<Option<ClaimedRow>, TransportError>> + Send;

    /// Read the next `bus_log` entry past `consumer`'s offset whose `name`
    /// matches one of `names` **or is NULL** (surfaced, not silently skipped,
    /// so the failure policy advances the offset past poison entries).
    fn log_read(
        &self,
        names: &[String],
        consumer: &str,
    ) -> impl Future<Output = Result<Option<ReceivedRow>, TransportError>> + Send;

    /// Delete a claimed queue row, fenced by its claim token.
    fn delete_claimed(
        &self,
        seq: i64,
        claim_token: &str,
    ) -> impl Future<Output = Result<(), TransportError>> + Send;

    /// Release a claimed queue row for redelivery, fenced by its claim token.
    fn release_claim(
        &self,
        seq: i64,
        claim_token: &str,
    ) -> impl Future<Output = Result<(), TransportError>> + Send;

    /// Advance `consumer`'s log offset to `seq` (monotonic upsert).
    fn advance_offset(
        &self,
        consumer: &str,
        seq: i64,
    ) -> impl Future<Output = Result<(), TransportError>> + Send;
}

/// SQL-backed [`Bus`] + [`BusConsumer`], generic over a [`SqlBusDialect`].
/// Cheap to clone (the dialect wraps a pool, which is an `Arc`).
#[derive(Clone)]
pub struct SqlBus<B> {
    dialect: B,
    topology: BusTopologyConfig,
    lease: Duration,
}

impl<B: SqlBusDialect> SqlBus<B> {
    pub(crate) fn from_dialect(dialect: B) -> Self {
        Self {
            dialect,
            topology: BusTopologyConfig::default(),
            lease: DEFAULT_LEASE,
        }
    }

    /// Set an explicit durable event subscription group.
    pub fn group(mut self, group: impl Into<String>) -> Self {
        self.topology = self.topology.group(group);
        self
    }

    /// Override the claim lease for `listen` (how long a claimed command stays
    /// invisible to other workers before it is eligible for redelivery).
    pub fn with_lease(mut self, lease: Duration) -> Self {
        self.lease = lease;
        self
    }

    /// Create the bus tables (`bus_queue`, `bus_log`, `bus_offset`) if absent.
    ///
    /// Called by `listen`/`subscribe`; producers must ensure the tables exist
    /// before `send`/`publish`, either by calling this or through migrations.
    pub async fn ensure_tables(&self) -> Result<(), TransportError> {
        for statement in B::SCHEMA.split(';') {
            let statement = statement.trim();
            if statement.is_empty() {
                continue;
            }
            self.dialect.execute_ddl(statement).await?;
        }
        Ok(())
    }
}

impl<B: SqlBusDialect> Bus for SqlBus<B> {
    async fn send_message(&self, message: Message) -> Result<(), TransportError> {
        self.dialect.insert_queue(&message).await
    }

    async fn publish_message(&self, message: Message) -> Result<(), TransportError> {
        self.dialect.insert_log(&message).await
    }
}

impl<B: SqlBusDialect> BusConsumer for SqlBus<B> {
    async fn listen<R: MessageRouter>(
        &self,
        router: Arc<R>,
        options: RunOptions,
    ) -> Result<(), TransportError> {
        self.ensure_tables().await?;
        let names = router.subscription_plan().commands;
        if names.is_empty() {
            return Ok(());
        }
        let source = SqlQueueSource {
            dialect: self.dialect.clone(),
            names,
            lease_secs: self.lease.as_secs_f64(),
        };
        run_source(router, source, options).await
    }

    async fn subscribe<R: MessageRouter>(
        &self,
        router: Arc<R>,
        options: RunOptions,
    ) -> Result<(), TransportError> {
        self.ensure_tables().await?;
        let names = router.subscription_plan().events;
        if names.is_empty() {
            return Ok(());
        }
        let group = self
            .topology
            .resolve_consumer_group(router.as_ref(), B::BACKEND)?;
        let source = SqlLogSource {
            dialect: self.dialect.clone(),
            names,
            consumer: group,
        };
        run_source(router, source, options).await
    }
}

/// Competing-consumer source over `bus_queue` (atomic claim under a lease).
struct SqlQueueSource<B> {
    dialect: B,
    names: Vec<String>,
    lease_secs: f64,
}

impl<B: SqlBusDialect> MessageSource for SqlQueueSource<B> {
    type Received = SqlQueueReceived<B>;

    async fn recv(&mut self) -> Result<Option<Self::Received>, TransportError> {
        Ok(self
            .dialect
            .claim(&self.names, self.lease_secs)
            .await?
            .map(|claimed| SqlQueueReceived {
                dialect: self.dialect.clone(),
                row: claimed.row,
                claim_token: claimed.claim_token,
            }))
    }
}

/// A claimed `bus_queue` row: `ack` deletes it (done); `nack` makes it available
/// again (redelivery); `dead_letter`/`park` delete it (stop redelivery). Every
/// settlement is fenced by the claim token, so a stale worker cannot settle a
/// row that was reclaimed after its lease expired.
pub struct SqlQueueReceived<B> {
    dialect: B,
    row: ReceivedRow,
    claim_token: String,
}

impl<B: SqlBusDialect> ReceivedMessage for SqlQueueReceived<B> {
    fn message(&self) -> &Message {
        &self.row.message
    }

    fn decode_error(&self) -> Option<&TransportError> {
        self.row.decode_error.as_ref()
    }

    async fn ack(self) -> Result<(), TransportError> {
        self.dialect
            .delete_claimed(self.row.seq, &self.claim_token)
            .await
    }

    async fn nack(self, _reason: &str) -> Result<(), TransportError> {
        self.dialect
            .release_claim(self.row.seq, &self.claim_token)
            .await
    }

    async fn dead_letter(self, _reason: &str) -> Result<(), TransportError> {
        self.dialect
            .delete_claimed(self.row.seq, &self.claim_token)
            .await
    }

    async fn park(self, _reason: &str) -> Result<(), TransportError> {
        self.dialect
            .delete_claimed(self.row.seq, &self.claim_token)
            .await
    }
}

/// Fan-out source over `bus_log`: reads the next entry past this consumer's
/// offset for its subscribed names, in `seq` order.
struct SqlLogSource<B> {
    dialect: B,
    names: Vec<String>,
    consumer: String,
}

impl<B: SqlBusDialect> MessageSource for SqlLogSource<B> {
    type Received = SqlLogReceived<B>;

    async fn recv(&mut self) -> Result<Option<Self::Received>, TransportError> {
        Ok(self
            .dialect
            .log_read(&self.names, &self.consumer)
            .await?
            .map(|row| SqlLogReceived {
                dialect: self.dialect.clone(),
                consumer: self.consumer.clone(),
                row,
            }))
    }
}

/// A `bus_log` entry: `ack` advances this consumer's offset to its `seq` (the
/// effectively-once point); `nack` leaves the offset (redelivery);
/// `dead_letter`/`park` advance past it (skip, don't get stuck).
pub struct SqlLogReceived<B> {
    dialect: B,
    consumer: String,
    row: ReceivedRow,
}

impl<B: SqlBusDialect> ReceivedMessage for SqlLogReceived<B> {
    fn message(&self) -> &Message {
        &self.row.message
    }

    fn decode_error(&self) -> Option<&TransportError> {
        self.row.decode_error.as_ref()
    }

    async fn ack(self) -> Result<(), TransportError> {
        self.dialect
            .advance_offset(&self.consumer, self.row.seq)
            .await
    }

    async fn nack(self, _reason: &str) -> Result<(), TransportError> {
        // Leave the offset unmoved so the entry is re-read on the next poll.
        Ok(())
    }

    async fn dead_letter(self, _reason: &str) -> Result<(), TransportError> {
        self.dialect
            .advance_offset(&self.consumer, self.row.seq)
            .await
    }

    async fn park(self, _reason: &str) -> Result<(), TransportError> {
        self.dialect
            .advance_offset(&self.consumer, self.row.seq)
            .await
    }
}
