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
//! The bus model itself (builders, sources, settlement, corruption handling) is
//! shared with the SQLite bus in `sql_bus_common`; this
//! module contributes only the Postgres dialect: `$n` placeholders, the `now()`
//! clock, `= ANY` array binding for name lists, and `gen_random_uuid()` claim
//! tokens.
//!
//! ## Why claim-lease, not sqlxmq (implementation note)
//!
//! Decision #8 of the locked spec names `sqlxmq` as the recommended work-queue
//! backend and keeps the claim-lease queue as the **no-extra-dependency
//! alternative**. At implementation time the claim-lease queue was chosen because
//! sqlxmq owns an always-on, push-based `JobRunner` loop, which does not compose
//! with the facade's uniform *drain-to-idle* [`run_source`] model that every other
//! `*Bus` (in-memory, NATS, RabbitMQ, Kafka) and their tests share — a claim-lease
//! [`MessageSource`] returns `Ok(None)` when the queue is empty and stops
//! cleanly. sqlxmq remains a viable future backend for its mature NOTIFY/backoff;
//! revisit if those are needed. See [[tasks/build-transport-bus-facade]].
//!
//! Requires the `postgres` feature. Integration-tested in `tests/postgres_transport`.
//!
//! [`Bus`]: super::Bus
//! [`BusConsumer`]: super::BusConsumer
//! [`MessageSource`]: super::MessageSource
//! [`run_source`]: super::run_source

use sqlx::{PgPool, Row};

use super::sql_bus_common::{
    db_err as sql_db_err, metadata_json, ClaimedRow, ReceivedRow, SqlBus, SqlBusDialect,
    SqlLogReceived, SqlQueueReceived,
};
use super::{Message, TransportError};

const SCHEMA: &str = "\
CREATE TABLE IF NOT EXISTS bus_queue (
    seq          BIGSERIAL PRIMARY KEY,
    claim_token  TEXT,
    name         TEXT NOT NULL,
    message_id   TEXT,
    kind         TEXT NOT NULL,
    payload      BYTEA NOT NULL,
    content_type TEXT NOT NULL DEFAULT 'application/json',
    metadata     TEXT NOT NULL DEFAULT '[]',
    available_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    locked_until TIMESTAMPTZ,
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
    seq          BIGSERIAL PRIMARY KEY,
    name         TEXT NOT NULL,
    message_id   TEXT,
    kind         TEXT NOT NULL,
    payload      BYTEA NOT NULL,
    content_type TEXT NOT NULL DEFAULT 'application/json',
    metadata     TEXT NOT NULL DEFAULT '[]',
    appended_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (name <> ''),
    CHECK (kind IN ('command', 'event')),
    CHECK (content_type <> '')
);
CREATE INDEX IF NOT EXISTS bus_log_name_seq_idx ON bus_log (name, seq);
CREATE TABLE IF NOT EXISTS bus_offset (
    consumer TEXT PRIMARY KEY,
    last_seq BIGINT NOT NULL DEFAULT 0,
    CHECK (consumer <> ''),
    CHECK (last_seq >= 0)
)";

fn db_err(context: &str, err: sqlx::Error) -> TransportError {
    sql_db_err(PostgresBusDialect::BACKEND, context, err)
}

/// Postgres [`Bus`](super::Bus) + [`BusConsumer`](super::BusConsumer). Cheap to
/// clone (the pool is an `Arc`).
pub type PostgresBus = SqlBus<PostgresBusDialect>;

/// A claimed Postgres `bus_queue` row: `ack` deletes it (done); `nack` makes it
/// available again (redelivery); `dead_letter`/`park` delete it (stop
/// redelivery). Settlement is fenced by the claim token.
pub type QueueReceived = SqlQueueReceived<PostgresBusDialect>;

/// A Postgres `bus_log` entry: `ack` advances this consumer's offset to its
/// `seq`; `nack` leaves the offset (redelivery); `dead_letter`/`park` advance
/// past it (skip, don't get stuck).
pub type LogReceived = SqlLogReceived<PostgresBusDialect>;

impl PostgresBus {
    /// Build a bus over an existing pool.
    ///
    /// For event subscriptions, `subscribe` uses the router's consumer identity as
    /// the durable Postgres log offset. Service consumers usually get that identity
    /// from [`Service::named`](crate::microsvc::Service::named). Direct consumers
    /// can set it with [`group`](Self::group). Commands are claimed from
    /// `bus_queue` by message name, so command replicas compete by listening to the
    /// same registered command names.
    pub fn new(pool: PgPool) -> Self {
        SqlBus::from_dialect(PostgresBusDialect { pool })
    }

    /// Build a bus with an explicit group for direct/low-level use.
    pub fn new_with_group(pool: PgPool, group: impl Into<String>) -> Self {
        Self::new(pool).group(group)
    }
}

/// The Postgres [`SqlBusDialect`].
#[derive(Clone)]
pub struct PostgresBusDialect {
    pool: PgPool,
}

impl PostgresBusDialect {
    async fn insert(
        &self,
        sql: &'static str,
        context: &'static str,
        message: &Message,
    ) -> Result<(), TransportError> {
        let metadata = metadata_json(message);
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

impl SqlBusDialect for PostgresBusDialect {
    const BACKEND: &'static str = "postgres";
    const SCHEMA: &'static str = SCHEMA;

    async fn execute_ddl(&self, statement: &'static str) -> Result<(), TransportError> {
        sqlx::query(statement)
            .execute(&self.pool)
            .await
            .map_err(|err| db_err("ensure_tables", err))?;
        Ok(())
    }

    async fn insert_queue(&self, message: &Message) -> Result<(), TransportError> {
        self.insert(
            "INSERT INTO bus_queue (name, message_id, kind, payload, content_type, metadata) \
             VALUES ($1, $2, $3, $4, $5, $6)",
            "enqueue",
            message,
        )
        .await
    }

    async fn insert_log(&self, message: &Message) -> Result<(), TransportError> {
        self.insert(
            "INSERT INTO bus_log (name, message_id, kind, payload, content_type, metadata) \
             VALUES ($1, $2, $3, $4, $5, $6)",
            "append",
            message,
        )
        .await
    }

    async fn claim(
        &self,
        names: &[String],
        lease_secs: f64,
        limit: i64,
    ) -> Result<Vec<ClaimedRow>, TransportError> {
        // `FOR UPDATE SKIP LOCKED` keeps the claim safe under competing
        // listeners — only one claims (and later settles) each row.
        // `gen_random_uuid()` is volatile: each claimed row gets its own token.
        let rows = sqlx::query(
            "UPDATE bus_queue SET locked_until = now() + ($1 * interval '1 second'), \
                    claim_token = gen_random_uuid()::text, \
                    attempts = attempts + 1 \
             WHERE seq IN ( \
                SELECT seq FROM bus_queue \
                WHERE (name = ANY($2) OR name IS NULL) AND available_at <= now() \
                      AND (locked_until IS NULL OR locked_until <= now()) \
                ORDER BY seq FOR UPDATE SKIP LOCKED LIMIT $3 \
             ) \
             RETURNING seq, claim_token, name, message_id, kind, payload, content_type, metadata",
        )
        .bind(lease_secs)
        .bind(names)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|err| db_err("claim", err))?;

        rows.into_iter()
            .map(|row| {
                let claim_token = row
                    .try_get("claim_token")
                    .map_err(|err| db_err("claim token", err))?;
                Ok(ClaimedRow {
                    row: ReceivedRow::from_row(Self::BACKEND, &row),
                    claim_token,
                })
            })
            .collect()
    }

    async fn log_read(
        &self,
        names: &[String],
        consumer: &str,
        limit: i64,
    ) -> Result<Vec<ReceivedRow>, TransportError> {
        let rows = sqlx::query(
            "SELECT seq, name, message_id, kind, payload, content_type, metadata FROM bus_log \
             WHERE (name = ANY($1) OR name IS NULL) \
                   AND seq > COALESCE((SELECT last_seq FROM bus_offset WHERE consumer = $2), 0) \
             ORDER BY seq LIMIT $3",
        )
        .bind(names)
        .bind(consumer)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|err| db_err("log read", err))?;

        Ok(rows
            .iter()
            .map(|row| ReceivedRow::from_row(Self::BACKEND, row))
            .collect())
    }

    async fn delete_claimed(&self, seq: i64, claim_token: &str) -> Result<(), TransportError> {
        sqlx::query("DELETE FROM bus_queue WHERE seq = $1 AND claim_token = $2")
            .bind(seq)
            .bind(claim_token)
            .execute(&self.pool)
            .await
            .map_err(|err| db_err("delete", err))?;
        Ok(())
    }

    async fn release_claim(&self, seq: i64, claim_token: &str) -> Result<(), TransportError> {
        sqlx::query(
            "UPDATE bus_queue \
             SET locked_until = NULL, claim_token = NULL \
             WHERE seq = $1 AND claim_token = $2",
        )
        .bind(seq)
        .bind(claim_token)
        .execute(&self.pool)
        .await
        .map_err(|err| db_err("nack", err))?;
        Ok(())
    }

    async fn advance_offset(&self, consumer: &str, seq: i64) -> Result<(), TransportError> {
        sqlx::query(
            "INSERT INTO bus_offset (consumer, last_seq) VALUES ($1, $2) \
             ON CONFLICT (consumer) DO UPDATE SET last_seq = EXCLUDED.last_seq \
             WHERE bus_offset.last_seq < EXCLUDED.last_seq",
        )
        .bind(consumer)
        .bind(seq)
        .execute(&self.pool)
        .await
        .map_err(|err| db_err("advance offset", err))?;
        Ok(())
    }
}
