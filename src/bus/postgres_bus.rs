//! Postgres [`Bus`] + [`BusConsumer`] — a complete single-DB bus.
//!
//! Postgres covers **both** bus modes (see [[specs/transport-bus-facade]]):
//!
//! - **`send` / `listen` (point-to-point, competing):** a durable work-queue
//!   table (`bus_queue`) claimed with `FOR UPDATE SKIP LOCKED` under a lease, so
//!   one of N competing `listen`ers handles each command and the row is deleted
//!   on success (redelivered on nack, until a `dead_letter`/`park` drops it).
//! - **`publish` / `subscribe` (fan-out):** Postgres modelled as a log — an
//!   append-only `bus_log` table (monotonic `seq`, retained) plus a per-consumer
//!   offset table (`bus_offset`: `consumer → last_seq`). `publish` appends; each
//!   `subscribe`r (keyed by its `group`) reads `seq > last_seq` for its event
//!   names in order and advances its own offset, so every group sees every event.
//!   Because the log, the offset, and projection writes share one Postgres, the
//!   offset advances in the same database as the effects — the cleanest path to
//!   transactional effectively-once of any transport (the offset is the inbox).
//!
//! ## Why claim-lease, not sqlxmq (implementation note)
//!
//! Decision #8 of the locked spec names `sqlxmq` as the recommended work-queue
//! backend and keeps the claim-lease queue as the **no-extra-dependency
//! alternative**. At implementation time the claim-lease queue was chosen because
//! sqlxmq owns an always-on, push-based `JobRunner` loop, which does not compose
//! with the facade's uniform *drain-to-idle* [`run_source`] model that every other
//! `*Bus` (in-memory, NATS, RabbitMQ, Kafka) and their tests share — a claim-lease
//! [`AsyncMessageSource`] returns `Ok(None)` when the queue is empty and stops
//! cleanly. sqlxmq remains a viable future backend for its mature NOTIFY/backoff;
//! revisit if those are needed. See [[tasks/build-transport-bus-facade]].
//!
//! Requires the `postgres` feature. Integration-tested in `tests/postgres_transport`.
//!
//! [`run_source`]: super::run_source

use std::sync::Arc;
use std::time::Duration;

use sqlx::{PgPool, Row};

use super::source::{AsyncMessageSource, ReceivedMessage};
use super::{run_source, Bus, BusConsumer, MessageRouter, RunOptions, TransportError};
use super::{Message, MessageKind};

const DEFAULT_LEASE: Duration = Duration::from_secs(30);

const SCHEMA: &str = "\
CREATE TABLE IF NOT EXISTS bus_queue (
    seq          BIGSERIAL PRIMARY KEY,
    name         TEXT NOT NULL,
    message_id   TEXT,
    kind         TEXT NOT NULL,
    payload      BYTEA NOT NULL,
    content_type TEXT NOT NULL DEFAULT 'application/json',
    metadata     TEXT NOT NULL DEFAULT '[]',
    available_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    locked_until TIMESTAMPTZ,
    attempts     INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS bus_queue_claim_idx ON bus_queue (name, available_at, seq);
CREATE TABLE IF NOT EXISTS bus_log (
    seq          BIGSERIAL PRIMARY KEY,
    name         TEXT NOT NULL,
    message_id   TEXT,
    kind         TEXT NOT NULL,
    payload      BYTEA NOT NULL,
    content_type TEXT NOT NULL DEFAULT 'application/json',
    metadata     TEXT NOT NULL DEFAULT '[]',
    appended_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS bus_log_name_seq_idx ON bus_log (name, seq);
CREATE TABLE IF NOT EXISTS bus_offset (
    consumer TEXT PRIMARY KEY,
    last_seq BIGINT NOT NULL DEFAULT 0
)";

fn db_err(context: &str, err: sqlx::Error) -> TransportError {
    // Database errors are transient from the transport's view; the runner retries.
    TransportError::retryable(format!("postgres bus {context}: {err}"))
}

fn kind_str(kind: MessageKind) -> &'static str {
    match kind {
        MessageKind::Command => "command",
        MessageKind::Event => "event",
    }
}

fn kind_from_str(value: &str) -> MessageKind {
    match value {
        "command" => MessageKind::Command,
        _ => MessageKind::Event,
    }
}

fn message_from_row(row: &sqlx::postgres::PgRow) -> Message {
    let metadata_json: String = row.try_get("metadata").unwrap_or_else(|_| "[]".to_string());
    let metadata =
        serde_json::from_str::<Vec<(String, String)>>(&metadata_json).unwrap_or_default();
    Message {
        id: row
            .try_get::<Option<String>, _>("message_id")
            .unwrap_or(None),
        name: row.try_get("name").unwrap_or_default(),
        kind: kind_from_str(&row.try_get::<String, _>("kind").unwrap_or_default()),
        payload: row.try_get("payload").unwrap_or_default(),
        content_type: row
            .try_get("content_type")
            .unwrap_or_else(|_| "application/json".to_string()),
        metadata,
    }
}

/// Postgres [`Bus`] + [`BusConsumer`]. Cheap to clone (the pool is an `Arc`).
#[derive(Clone)]
pub struct PostgresBus {
    pool: PgPool,
    group: String,
    lease: Duration,
}

impl PostgresBus {
    /// Build a bus over an existing pool. `group` is the consumer identity:
    /// replicas sharing a `group` compete on the queue (point-to-point) and share
    /// one log offset; distinct `group`s each get their own log offset (fan-out).
    pub fn new(pool: PgPool, group: impl Into<String>) -> Self {
        Self {
            pool,
            group: group.into(),
            lease: DEFAULT_LEASE,
        }
    }

    /// Override the claim lease for `listen` (how long a claimed command stays
    /// invisible to other workers before it is eligible for redelivery).
    pub fn with_lease(mut self, lease: Duration) -> Self {
        self.lease = lease;
        self
    }

    /// Create the bus tables (`bus_queue`, `bus_log`, `bus_offset`) if absent, in
    /// the pool's current schema. Called by `listen`/`subscribe`; producers must
    /// ensure the tables exist (here or via migration) before `send`/`publish`.
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
        let metadata = serde_json::to_string(&message.metadata).unwrap_or_else(|_| "[]".into());
        sqlx::query(
            "INSERT INTO bus_queue (name, message_id, kind, payload, content_type, metadata) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(&message.name)
        .bind(&message.id)
        .bind(kind_str(message.kind))
        .bind(&message.payload)
        .bind(&message.content_type)
        .bind(metadata)
        .execute(&self.pool)
        .await
        .map_err(|err| db_err("enqueue", err))?;
        Ok(())
    }

    async fn append(&self, message: Message) -> Result<(), TransportError> {
        let metadata = serde_json::to_string(&message.metadata).unwrap_or_else(|_| "[]".into());
        sqlx::query(
            "INSERT INTO bus_log (name, message_id, kind, payload, content_type, metadata) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(&message.name)
        .bind(&message.id)
        .bind(kind_str(message.kind))
        .bind(&message.payload)
        .bind(&message.content_type)
        .bind(metadata)
        .execute(&self.pool)
        .await
        .map_err(|err| db_err("append", err))?;
        Ok(())
    }
}

impl Bus for PostgresBus {
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

impl BusConsumer for PostgresBus {
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
        let source = LogSource {
            pool: self.pool.clone(),
            names,
            consumer: self.group.clone(),
        };
        run_source(router, source, options).await
    }
}

/// Competing-consumer source over `bus_queue` (`FOR UPDATE SKIP LOCKED` claim).
struct QueueSource {
    pool: PgPool,
    names: Vec<String>,
    lease_secs: f64,
}

impl AsyncMessageSource for QueueSource {
    type Received = QueueReceived;

    async fn recv(&mut self) -> Result<Option<Self::Received>, TransportError> {
        let row = sqlx::query(
            "UPDATE bus_queue SET locked_until = now() + ($1 * interval '1 second'), \
                    attempts = attempts + 1 \
             WHERE seq = ( \
                SELECT seq FROM bus_queue \
                WHERE name = ANY($2) AND available_at <= now() \
                      AND (locked_until IS NULL OR locked_until < now()) \
                ORDER BY seq FOR UPDATE SKIP LOCKED LIMIT 1 \
             ) \
             RETURNING seq, name, message_id, kind, payload, content_type, metadata",
        )
        .bind(self.lease_secs)
        .bind(&self.names)
        .fetch_optional(&self.pool)
        .await
        .map_err(|err| db_err("claim", err))?;

        Ok(row.map(|row| {
            let seq: i64 = row.try_get("seq").unwrap_or_default();
            QueueReceived {
                pool: self.pool.clone(),
                seq,
                message: message_from_row(&row),
            }
        }))
    }
}

/// A claimed `bus_queue` row: `ack` deletes it (done); `nack` makes it available
/// again (redelivery); `dead_letter`/`park` delete it (stop redelivery).
pub struct QueueReceived {
    pool: PgPool,
    seq: i64,
    message: Message,
}

impl QueueReceived {
    async fn delete(&self) -> Result<(), TransportError> {
        sqlx::query("DELETE FROM bus_queue WHERE seq = $1")
            .bind(self.seq)
            .execute(&self.pool)
            .await
            .map_err(|err| db_err("delete", err))?;
        Ok(())
    }
}

impl ReceivedMessage for QueueReceived {
    fn message(&self) -> &Message {
        &self.message
    }

    async fn ack(self) -> Result<(), TransportError> {
        self.delete().await
    }

    async fn nack(self, _reason: &str) -> Result<(), TransportError> {
        sqlx::query("UPDATE bus_queue SET locked_until = NULL WHERE seq = $1")
            .bind(self.seq)
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

/// Fan-out source over `bus_log`: reads the next entry past this consumer's
/// offset for its subscribed names, in `seq` order.
struct LogSource {
    pool: PgPool,
    names: Vec<String>,
    consumer: String,
}

impl AsyncMessageSource for LogSource {
    type Received = LogReceived;

    async fn recv(&mut self) -> Result<Option<Self::Received>, TransportError> {
        let row = sqlx::query(
            "SELECT seq, name, message_id, kind, payload, content_type, metadata FROM bus_log \
             WHERE name = ANY($1) \
                   AND seq > COALESCE((SELECT last_seq FROM bus_offset WHERE consumer = $2), 0) \
             ORDER BY seq LIMIT 1",
        )
        .bind(&self.names)
        .bind(&self.consumer)
        .fetch_optional(&self.pool)
        .await
        .map_err(|err| db_err("log read", err))?;

        Ok(row.map(|row| {
            let seq: i64 = row.try_get("seq").unwrap_or_default();
            LogReceived {
                pool: self.pool.clone(),
                consumer: self.consumer.clone(),
                seq,
                message: message_from_row(&row),
            }
        }))
    }
}

/// A `bus_log` entry: `ack` advances this consumer's offset to its `seq` (the
/// effectively-once point); `nack` leaves the offset (redelivery);
/// `dead_letter`/`park` advance past it (skip, don't get stuck).
pub struct LogReceived {
    pool: PgPool,
    consumer: String,
    seq: i64,
    message: Message,
}

impl LogReceived {
    async fn advance_offset(&self) -> Result<(), TransportError> {
        sqlx::query(
            "INSERT INTO bus_offset (consumer, last_seq) VALUES ($1, $2) \
             ON CONFLICT (consumer) DO UPDATE SET last_seq = EXCLUDED.last_seq \
             WHERE bus_offset.last_seq < EXCLUDED.last_seq",
        )
        .bind(&self.consumer)
        .bind(self.seq)
        .execute(&self.pool)
        .await
        .map_err(|err| db_err("advance offset", err))?;
        Ok(())
    }
}

impl ReceivedMessage for LogReceived {
    fn message(&self) -> &Message {
        &self.message
    }

    async fn ack(self) -> Result<(), TransportError> {
        self.advance_offset().await
    }

    async fn nack(self, _reason: &str) -> Result<(), TransportError> {
        // Leave the offset unmoved so the entry is re-read on the next poll.
        Ok(())
    }

    async fn dead_letter(self, _reason: &str) -> Result<(), TransportError> {
        self.advance_offset().await
    }

    async fn park(self, _reason: &str) -> Result<(), TransportError> {
        self.advance_offset().await
    }
}
