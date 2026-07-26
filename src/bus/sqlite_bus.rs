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
//! The bus model itself (builders, sources, settlement, corruption handling) is
//! shared with the Postgres bus in `sql_bus_common`;
//! this module contributes only the SQLite dialect: `?` placeholders, the
//! `unixepoch('now','subsec')` clock, `IN`-list name binding, and
//! `randomblob(16)` claim tokens.
//!
//! This is a no-extra-process transport for local development, tests, demos, and
//! small single-node deployments. It is not a high-throughput broker replacement.
//!
//! Requires the `sqlite` feature. Integration-tested in `tests/sqlite_transport`.
//!
//! [`Bus`]: super::Bus
//! [`BusConsumer`]: super::BusConsumer

use sqlx::{QueryBuilder, Row, Sqlite, SqlitePool};

use super::sql_bus_common::{
    db_err as sql_db_err, metadata_json, ClaimedRow, ReceivedRow, SqlBus, SqlBusDialect,
    SqlLogReceived, SqlQueueReceived,
};
use super::{Message, TransportError};

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
    sql_db_err(SqliteBusDialect::BACKEND, context, err)
}

/// Push `(name IN (…) OR name IS NULL)` with one bind per name.
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

/// SQLite [`Bus`](super::Bus) + [`BusConsumer`](super::BusConsumer). Cheap to
/// clone (the pool is an `Arc`).
pub type SqliteBus = SqlBus<SqliteBusDialect>;

/// A claimed SQLite `bus_queue` row: `ack` deletes it (done); `nack` makes it
/// available again (redelivery); `dead_letter`/`park` delete it (stop
/// redelivery). Settlement is fenced by the claim token.
pub type SqliteQueueReceived = SqlQueueReceived<SqliteBusDialect>;

/// A SQLite `bus_log` entry: `ack` advances this consumer's offset to its
/// `seq`; `nack` leaves the offset (redelivery); `dead_letter`/`park` advance
/// past it (skip, don't get stuck).
pub type SqliteLogReceived = SqlLogReceived<SqliteBusDialect>;

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
        SqlBus::from_dialect(SqliteBusDialect { pool })
    }

    /// Build a bus with an explicit group for direct/low-level use.
    pub fn new_with_group(pool: SqlitePool, group: impl Into<String>) -> Self {
        Self::new(pool).group(group)
    }
}

/// The SQLite [`SqlBusDialect`].
#[derive(Clone)]
pub struct SqliteBusDialect {
    pool: SqlitePool,
}

impl SqliteBusDialect {
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

impl SqlBusDialect for SqliteBusDialect {
    const BACKEND: &'static str = "sqlite";
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
             VALUES (?, ?, ?, ?, ?, ?)",
            "enqueue",
            message,
        )
        .await
    }

    async fn insert_log(&self, message: &Message) -> Result<(), TransportError> {
        self.insert(
            "INSERT INTO bus_log (name, message_id, kind, payload, content_type, metadata) \
             VALUES (?, ?, ?, ?, ?, ?)",
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
        // SQLite serializes writers, so the `UPDATE ... RETURNING` claim is
        // atomic under competing listeners — only one claims each row.
        // `randomblob` is non-deterministic: each claimed row gets its own token.
        let mut query = QueryBuilder::<Sqlite>::new(
            "UPDATE bus_queue \
             SET locked_until = unixepoch('now','subsec') + ",
        );
        query.push_bind(lease_secs);
        query.push(
            ", claim_token = lower(hex(randomblob(16))), \
                attempts = attempts + 1 \
             WHERE seq IN ( \
                SELECT seq FROM bus_queue \
                WHERE ",
        );
        push_name_filter(&mut query, names);
        query.push(
            " AND available_at <= unixepoch('now','subsec') \
              AND (locked_until IS NULL OR locked_until <= unixepoch('now','subsec')) \
              ORDER BY seq LIMIT ",
        );
        query.push_bind(limit);
        query.push(
            ") \
             RETURNING seq, claim_token, name, message_id, kind, payload, content_type, metadata, attempts",
        );

        let rows = query
            .build()
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
        let mut query = QueryBuilder::<Sqlite>::new(
            "SELECT seq, name, message_id, kind, payload, content_type, metadata, \
                    appended_at AS producer_timestamp \
             FROM bus_log \
             WHERE ",
        );
        push_name_filter(&mut query, names);
        query.push(" AND seq > COALESCE((SELECT last_seq FROM bus_offset WHERE consumer = ");
        query.push_bind(consumer);
        query.push("), 0) ORDER BY seq LIMIT ");
        query.push_bind(limit);

        let rows = query
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|err| db_err("log read", err))?;

        Ok(rows
            .iter()
            .map(|row| ReceivedRow::from_row(Self::BACKEND, row))
            .collect())
    }

    async fn delete_claimed(&self, seq: i64, claim_token: &str) -> Result<(), TransportError> {
        sqlx::query("DELETE FROM bus_queue WHERE seq = ? AND claim_token = ?")
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
             WHERE seq = ? AND claim_token = ?",
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
            "INSERT INTO bus_offset (consumer, last_seq) VALUES (?, ?) \
             ON CONFLICT (consumer) DO UPDATE SET last_seq = excluded.last_seq \
             WHERE bus_offset.last_seq < excluded.last_seq",
        )
        .bind(consumer)
        .bind(seq)
        .execute(&self.pool)
        .await
        .map_err(|err| db_err("advance offset", err))?;
        Ok(())
    }
}
