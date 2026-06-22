//! SQLite [`Bus`] + [`BusConsumer`] — a local durable bus.
//!
//! SQLite covers both bus modes with the same app-facing shape as
//! [`PostgresBus`](super::PostgresBus), but with SQLite-native concurrency:
//!
//! - **`send` / `listen` (point-to-point, competing):** a durable work-queue
//!   table (`bus_queue`) claimed by one atomic `UPDATE ... RETURNING` under a
//!   lease. SQLite serializes writers, so one of N competing `listen`ers handles
//!   each command and the row is deleted on success.
//! - **`publish` / `subscribe` (fan-out):** an append-only `bus_log` table plus
//!   a per-consumer offset table (`bus_offset`). Each `group` reads independently
//!   past its own offset, so every group sees every event.
//!
//! This is a no-extra-process transport for local development, tests, demos, and
//! small single-node deployments. It is not a high-throughput broker replacement.
//!
//! Requires the `sqlite` feature. Integration-tested in `tests/sqlite_transport`.

use std::sync::Arc;
use std::time::Duration;

use sqlx::sqlite::SqliteRow;
use sqlx::{QueryBuilder, Row, Sqlite, SqlitePool};

use crate::sqlx_repo::is_sqlx_transient;

use super::source::{MessageSource, ReceivedMessage};
use super::{
    run_source, Bus, BusConsumer, BusTopologyConfig, MessageRouter, RunOptions, TransportError,
    TransportErrorKind,
};
use super::{Message, MessageKind};

const DEFAULT_LEASE: Duration = Duration::from_secs(30);

const SCHEMA: &str = "\
CREATE TABLE IF NOT EXISTS bus_queue (
    seq          INTEGER PRIMARY KEY AUTOINCREMENT,
    claim_token  TEXT,
    name         TEXT NOT NULL,
    message_id   TEXT,
    kind         TEXT NOT NULL,
    payload      BLOB NOT NULL,
    content_type TEXT NOT NULL DEFAULT 'application/json',
    metadata     TEXT NOT NULL DEFAULT '[]',
    available_at REAL NOT NULL DEFAULT (unixepoch('now','subsec')),
    locked_until REAL,
    attempts     INTEGER NOT NULL DEFAULT 0,
    CHECK (claim_token IS NULL OR claim_token <> ''),
    CHECK (name <> ''),
    CHECK (kind IN ('command', 'event')),
    CHECK (content_type <> ''),
    CHECK (attempts >= 0)
);
CREATE INDEX IF NOT EXISTS bus_queue_claim_idx
    ON bus_queue (name, available_at, locked_until, seq);
CREATE TABLE IF NOT EXISTS bus_log (
    seq          INTEGER PRIMARY KEY AUTOINCREMENT,
    name         TEXT NOT NULL,
    message_id   TEXT,
    kind         TEXT NOT NULL,
    payload      BLOB NOT NULL,
    content_type TEXT NOT NULL DEFAULT 'application/json',
    metadata     TEXT NOT NULL DEFAULT '[]',
    appended_at  REAL NOT NULL DEFAULT (unixepoch('now','subsec')),
    CHECK (name <> ''),
    CHECK (kind IN ('command', 'event')),
    CHECK (content_type <> '')
);
CREATE INDEX IF NOT EXISTS bus_log_name_seq_idx ON bus_log (name, seq);
CREATE TABLE IF NOT EXISTS bus_offset (
    consumer TEXT PRIMARY KEY,
    last_seq INTEGER NOT NULL DEFAULT 0,
    CHECK (consumer <> ''),
    CHECK (last_seq >= 0)
)";

fn db_err(context: &str, err: sqlx::Error) -> TransportError {
    let kind = if is_sqlx_transient(&err) {
        TransportErrorKind::Retryable
    } else {
        TransportErrorKind::Permanent
    };
    TransportError::new(kind, format!("sqlite bus {context}: {err}")).with_source(err)
}

fn push_name_filter(query: &mut QueryBuilder<Sqlite>, names: &[String]) {
    query.push("(name IN (");
    {
        let mut separated = query.separated(", ");
        for name in names {
            separated.push_bind(name.as_str());
        }
    }
    query.push(") OR name IS NULL)");
}

fn metadata_json(message: &Message) -> String {
    serde_json::to_string(&message.metadata).unwrap_or_else(|_| "[]".into())
}

fn corrupt_row(message: impl Into<String>) -> TransportError {
    TransportError::permanent(format!("sqlite bus corrupt row: {}", message.into()))
}

fn decode_err(column: &str, err: sqlx::Error) -> TransportError {
    corrupt_row(format!(
        "required column '{column}' failed to decode: {err}"
    ))
}

fn parse_message_kind(value: &str) -> Result<MessageKind, TransportError> {
    match value {
        "command" => Ok(MessageKind::Command),
        "event" => Ok(MessageKind::Event),
        _ => Err(corrupt_row(format!(
            "required column 'kind' has unsupported value {value:?}"
        ))),
    }
}

fn parse_metadata(value: &str) -> Result<Vec<(String, String)>, TransportError> {
    serde_json::from_str(value).map_err(|err| {
        corrupt_row(format!(
            "required column 'metadata' failed to parse as JSON metadata: {err}"
        ))
    })
}

/// Reconstruct a [`Message`] from a claimed `bus_queue`/`bus_log` row.
///
/// Required-column decode/parsing failures are permanent corruption. The row has
/// already been selected or claimed, so callers surface the error through
/// [`ReceivedMessage::decode_error`] and let the runner settle it through the
/// configured failure policy.
fn message_from_row(row: &SqliteRow) -> Result<Message, TransportError> {
    let name: String = row.try_get("name").map_err(|err| decode_err("name", err))?;
    let kind: String = row.try_get("kind").map_err(|err| decode_err("kind", err))?;
    let payload: Vec<u8> = row
        .try_get("payload")
        .map_err(|err| decode_err("payload", err))?;
    let content_type: String = row
        .try_get("content_type")
        .map_err(|err| decode_err("content_type", err))?;
    if content_type.is_empty() {
        return Err(corrupt_row("required column 'content_type' is empty"));
    }
    let metadata_json: String = row
        .try_get("metadata")
        .map_err(|err| decode_err("metadata", err))?;
    let metadata = parse_metadata(&metadata_json)?;

    Ok(Message {
        id: row
            .try_get::<Option<String>, _>("message_id")
            .unwrap_or(None),
        name,
        kind: parse_message_kind(&kind)?,
        payload,
        content_type,
        metadata,
    })
}

struct ReceivedRow {
    seq: i64,
    message: Message,
    decode_error: Option<TransportError>,
}

impl ReceivedRow {
    fn from_row(row: &SqliteRow) -> Self {
        let seq = row.try_get("seq").unwrap_or_default();
        let (message, decode_error) = decode_or_placeholder(row);
        Self {
            seq,
            message,
            decode_error,
        }
    }

    fn message(&self) -> &Message {
        &self.message
    }

    fn decode_error(&self) -> Option<&TransportError> {
        self.decode_error.as_ref()
    }
}

/// SQLite [`Bus`] + [`BusConsumer`]. Cheap to clone (the pool is an `Arc`).
#[derive(Clone)]
pub struct SqliteBus {
    pool: SqlitePool,
    topology: BusTopologyConfig,
    lease: Duration,
}

impl SqliteBus {
    /// Build a bus over an existing pool.
    ///
    /// For event subscriptions, `subscribe` uses the router's consumer identity
    /// as the durable SQLite log offset. Service consumers usually get that
    /// identity from [`Service::named`](crate::microsvc::Service::named). Direct
    /// consumers can set it with [`group`](Self::group). Commands are claimed
    /// from `bus_queue` by message name, so command replicas compete by listening
    /// to the same registered command names.
    pub fn new(pool: SqlitePool) -> Self {
        Self {
            pool,
            topology: BusTopologyConfig::default(),
            lease: DEFAULT_LEASE,
        }
    }

    /// Build a bus with an explicit group for direct/low-level use.
    pub fn new_with_group(pool: SqlitePool, group: impl Into<String>) -> Self {
        Self::new(pool).group(group)
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
        for statement in SCHEMA.split(';') {
            let statement = statement.trim();
            if statement.is_empty() {
                continue;
            }
            sqlx::query(statement)
                .execute(&self.pool)
                .await
                .map_err(|err| db_err("ensure_tables", err))?;
        }
        Ok(())
    }

    async fn enqueue(&self, message: Message) -> Result<(), TransportError> {
        self.insert_message(
            "INSERT INTO bus_queue (name, message_id, kind, payload, content_type, metadata) \
             VALUES (?, ?, ?, ?, ?, ?)",
            "enqueue",
            message,
        )
        .await
    }

    async fn append(&self, message: Message) -> Result<(), TransportError> {
        self.insert_message(
            "INSERT INTO bus_log (name, message_id, kind, payload, content_type, metadata) \
             VALUES (?, ?, ?, ?, ?, ?)",
            "append",
            message,
        )
        .await
    }

    async fn insert_message(
        &self,
        sql: &'static str,
        context: &'static str,
        message: Message,
    ) -> Result<(), TransportError> {
        let metadata = metadata_json(&message);
        sqlx::query(sql)
            .bind(&message.name)
            .bind(&message.id)
            .bind(message.kind.as_str())
            .bind(&message.payload)
            .bind(&message.content_type)
            .bind(metadata)
            .execute(&self.pool)
            .await
            .map_err(|err| db_err(context, err))?;
        Ok(())
    }
}

impl Bus for SqliteBus {
    async fn send(&self, name: &str, payload: Vec<u8>) -> Result<(), TransportError> {
        self.enqueue(Message::new(name, MessageKind::Command, payload))
            .await
    }

    async fn publish(&self, name: &str, payload: Vec<u8>) -> Result<(), TransportError> {
        self.append(Message::new(name, MessageKind::Event, payload))
            .await
    }

    async fn send_message(&self, message: Message) -> Result<(), TransportError> {
        self.enqueue(message).await
    }

    async fn publish_message(&self, message: Message) -> Result<(), TransportError> {
        self.append(message).await
    }
}

impl BusConsumer for SqliteBus {
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
        let source = QueueSource {
            pool: self.pool.clone(),
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
            .resolve_consumer_group(router.as_ref(), "sqlite")?;
        let source = LogSource {
            pool: self.pool.clone(),
            names,
            consumer: group,
        };
        run_source(router, source, options).await
    }
}

/// Competing-consumer source over `bus_queue`.
struct QueueSource {
    pool: SqlitePool,
    names: Vec<String>,
    lease_secs: f64,
}

impl MessageSource for QueueSource {
    type Received = SqliteQueueReceived;

    async fn recv(&mut self) -> Result<Option<Self::Received>, TransportError> {
        let mut query = QueryBuilder::<Sqlite>::new(
            "UPDATE bus_queue \
             SET locked_until = unixepoch('now','subsec') + ",
        );
        query.push_bind(self.lease_secs);
        query.push(
            ", claim_token = lower(hex(randomblob(16))), \
                attempts = attempts + 1 \
             WHERE seq = ( \
                SELECT seq FROM bus_queue \
                WHERE ",
        );
        push_name_filter(&mut query, &self.names);
        query.push(
            " AND available_at <= unixepoch('now','subsec') \
              AND (locked_until IS NULL OR locked_until <= unixepoch('now','subsec')) \
              ORDER BY seq LIMIT 1 \
             ) \
             RETURNING seq, claim_token, name, message_id, kind, payload, content_type, metadata",
        );

        let row = query
            .build()
            .fetch_optional(&self.pool)
            .await
            .map_err(|err| db_err("claim", err))?;

        if let Some(row) = row {
            let claim_token = row
                .try_get("claim_token")
                .map_err(|err| db_err("claim token", err))?;
            Ok(Some(SqliteQueueReceived {
                pool: self.pool.clone(),
                row: ReceivedRow::from_row(&row),
                claim_token,
            }))
        } else {
            Ok(None)
        }
    }
}

fn decode_or_placeholder(row: &SqliteRow) -> (Message, Option<TransportError>) {
    match message_from_row(row) {
        Ok(message) => (message, None),
        Err(error) => (
            Message::new("", MessageKind::Event, Vec::new()),
            Some(error),
        ),
    }
}

/// A claimed `bus_queue` row.
///
/// `ack` deletes it, `nack` releases the lease for redelivery, and
/// `dead_letter`/`park` delete it so poison rows do not redeliver forever.
pub struct SqliteQueueReceived {
    pool: SqlitePool,
    row: ReceivedRow,
    claim_token: String,
}

impl SqliteQueueReceived {
    async fn delete(&self) -> Result<(), TransportError> {
        sqlx::query("DELETE FROM bus_queue WHERE seq = ? AND claim_token = ?")
            .bind(self.row.seq)
            .bind(&self.claim_token)
            .execute(&self.pool)
            .await
            .map_err(|err| db_err("delete", err))?;
        Ok(())
    }
}

impl ReceivedMessage for SqliteQueueReceived {
    fn message(&self) -> &Message {
        self.row.message()
    }

    fn decode_error(&self) -> Option<&TransportError> {
        self.row.decode_error()
    }

    async fn ack(self) -> Result<(), TransportError> {
        self.delete().await
    }

    async fn nack(self, _reason: &str) -> Result<(), TransportError> {
        sqlx::query(
            "UPDATE bus_queue \
             SET locked_until = NULL, claim_token = NULL \
             WHERE seq = ? AND claim_token = ?",
        )
        .bind(self.row.seq)
        .bind(&self.claim_token)
        .execute(&self.pool)
        .await
        .map_err(|err| db_err("nack", err))?;
        Ok(())
    }

    async fn dead_letter(self, _reason: &str) -> Result<(), TransportError> {
        self.delete().await
    }

    async fn park(self, _reason: &str) -> Result<(), TransportError> {
        self.delete().await
    }
}

/// Fan-out source over `bus_log`.
struct LogSource {
    pool: SqlitePool,
    names: Vec<String>,
    consumer: String,
}

impl MessageSource for LogSource {
    type Received = SqliteLogReceived;

    async fn recv(&mut self) -> Result<Option<Self::Received>, TransportError> {
        let mut query = QueryBuilder::<Sqlite>::new(
            "SELECT seq, name, message_id, kind, payload, content_type, metadata \
             FROM bus_log \
             WHERE ",
        );
        push_name_filter(&mut query, &self.names);
        query.push(" AND seq > COALESCE((SELECT last_seq FROM bus_offset WHERE consumer = ");
        query.push_bind(&self.consumer);
        query.push("), 0) ORDER BY seq LIMIT 1");

        let row = query
            .build()
            .fetch_optional(&self.pool)
            .await
            .map_err(|err| db_err("log read", err))?;

        Ok(row.map(|row| SqliteLogReceived {
            pool: self.pool.clone(),
            consumer: self.consumer.clone(),
            row: ReceivedRow::from_row(&row),
        }))
    }
}

/// A `bus_log` entry.
///
/// `ack` advances this consumer's offset, `nack` leaves the offset unchanged,
/// and `dead_letter`/`park` advance past poison entries.
pub struct SqliteLogReceived {
    pool: SqlitePool,
    consumer: String,
    row: ReceivedRow,
}

impl SqliteLogReceived {
    async fn advance_offset(&self) -> Result<(), TransportError> {
        sqlx::query(
            "INSERT INTO bus_offset (consumer, last_seq) VALUES (?, ?) \
             ON CONFLICT (consumer) DO UPDATE SET last_seq = excluded.last_seq \
             WHERE bus_offset.last_seq < excluded.last_seq",
        )
        .bind(&self.consumer)
        .bind(self.row.seq)
        .execute(&self.pool)
        .await
        .map_err(|err| db_err("advance offset", err))?;
        Ok(())
    }
}

impl ReceivedMessage for SqliteLogReceived {
    fn message(&self) -> &Message {
        self.row.message()
    }

    fn decode_error(&self) -> Option<&TransportError> {
        self.row.decode_error()
    }

    async fn ack(self) -> Result<(), TransportError> {
        self.advance_offset().await
    }

    async fn nack(self, _reason: &str) -> Result<(), TransportError> {
        Ok(())
    }

    async fn dead_letter(self, _reason: &str) -> Result<(), TransportError> {
        self.advance_offset().await
    }

    async fn park(self, _reason: &str) -> Result<(), TransportError> {
        self.advance_offset().await
    }
}
