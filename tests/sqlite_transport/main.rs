//! SQLite bus transport integration tests.
//!
//! Exercises `SqliteBus` over a local SQLite file: command work-queue claims,
//! event log offsets, retry/nack behavior, and corrupt-row handling.
#![cfg(feature = "sqlite")]

// Shared broker-test helpers (recording_for, named_recording_for).
#[path = "../transport_conformance/mod.rs"]
mod conformance;
use conformance::{named_recording_for, recording_for};

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use distributed::bus::{
    Bus, BusConsumer, Handlers, Message, MessageKind, RunOptions, SqliteBus, TransportError,
};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;

static DB_SEQ: AtomicU64 = AtomicU64::new(0);

const COMMAND_NAME: &str = "order.initialize";
const EVENT_NAME: &str = "order.initialized";
const PAYLOAD: &[u8] = b"{}";

struct TempDb {
    path: PathBuf,
}

impl TempDb {
    fn new() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let seq = DB_SEQ.fetch_add(1, Ordering::Relaxed);
        let mut path = std::env::temp_dir();
        path.push(format!("distributed_sqlite_bus_test_{nanos}_{seq}.db"));
        Self { path }
    }

    async fn pool(&self) -> SqlitePool {
        self.pool_with_timeout(Duration::from_secs(5)).await
    }

    async fn pool_with_timeout(&self, busy_timeout: Duration) -> SqlitePool {
        let options = SqliteConnectOptions::new()
            .filename(&self.path)
            .create_if_missing(true)
            .busy_timeout(busy_timeout);
        SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(options)
            .await
            .expect("sqlite test pool")
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        for suffix in ["-wal", "-shm"] {
            let mut sidecar = self.path.clone();
            let name = format!("{}{suffix}", sidecar.file_name().unwrap().to_string_lossy());
            sidecar.set_file_name(name);
            let _ = std::fs::remove_file(sidecar);
        }
    }
}

async fn bus() -> (TempDb, SqlitePool, SqliteBus) {
    let db = TempDb::new();
    let pool = db.pool().await;
    let bus = SqliteBus::new(pool.clone()).group("orders");
    bus.ensure_tables().await.expect("ensure tables");
    (db, pool, bus)
}

fn command(id: impl Into<String>) -> Message {
    Message::new(COMMAND_NAME, MessageKind::Command, PAYLOAD.to_vec()).with_id(id)
}

fn event(id: impl Into<String>) -> Message {
    Message::new(EVENT_NAME, MessageKind::Event, PAYLOAD.to_vec()).with_id(id)
}

fn expected_ids(prefix: &str, total: usize) -> Vec<String> {
    (0..total).map(|i| format!("{prefix}{i}")).collect()
}

fn recorded_ids(rec: &Arc<Mutex<Vec<String>>>) -> Vec<String> {
    let mut ids = rec.lock().unwrap().clone();
    ids.sort();
    ids
}

async fn send_commands(bus: &SqliteBus, total: usize) {
    for message in expected_ids("c", total).into_iter().map(command) {
        bus.send_message(message).await.expect("send command");
    }
}

async fn publish_events(bus: &SqliteBus, total: usize) {
    for message in expected_ids("e", total).into_iter().map(event) {
        bus.publish_message(message).await.expect("publish event");
    }
}

async fn recreate_nullable_queue_table(pool: &SqlitePool) {
    sqlx::query("DROP TABLE IF EXISTS bus_queue")
        .execute(pool)
        .await
        .expect("drop bus_queue");
    sqlx::query(
        r#"
        CREATE TABLE bus_queue (
            seq          INTEGER PRIMARY KEY AUTOINCREMENT,
            name         TEXT,
            message_id   TEXT,
            kind         TEXT NOT NULL,
            payload      BLOB NOT NULL,
            content_type TEXT NOT NULL DEFAULT 'application/json',
            metadata     TEXT NOT NULL DEFAULT '[]',
            available_at REAL NOT NULL DEFAULT (unixepoch('now','subsec')),
            locked_until REAL,
            attempts     INTEGER NOT NULL DEFAULT 0
        )
        "#,
    )
    .execute(pool)
    .await
    .expect("create nullable bus_queue");
    sqlx::query(
        "CREATE INDEX bus_queue_claim_idx ON bus_queue (name, available_at, locked_until, seq)",
    )
    .execute(pool)
    .await
    .expect("create queue index");
}

async fn recreate_nullable_log_table(pool: &SqlitePool) {
    sqlx::query("DROP TABLE IF EXISTS bus_log")
        .execute(pool)
        .await
        .expect("drop bus_log");
    sqlx::query(
        r#"
        CREATE TABLE bus_log (
            seq          INTEGER PRIMARY KEY AUTOINCREMENT,
            name         TEXT,
            message_id   TEXT,
            kind         TEXT NOT NULL,
            payload      BLOB NOT NULL,
            content_type TEXT NOT NULL DEFAULT 'application/json',
            metadata     TEXT NOT NULL DEFAULT '[]',
            appended_at  REAL NOT NULL DEFAULT (unixepoch('now','subsec'))
        )
        "#,
    )
    .execute(pool)
    .await
    .expect("create nullable bus_log");
    sqlx::query("CREATE INDEX bus_log_name_seq_idx ON bus_log (name, seq)")
        .execute(pool)
        .await
        .expect("create log index");
}

async fn corrupt_latest_queue_name(pool: &SqlitePool) {
    sqlx::query("UPDATE bus_queue SET name = NULL WHERE seq = (SELECT max(seq) FROM bus_queue)")
        .execute(pool)
        .await
        .expect("null out queue name");
}

async fn corrupt_latest_log_name(pool: &SqlitePool) {
    sqlx::query("UPDATE bus_log SET name = NULL WHERE seq = (SELECT max(seq) FROM bus_log)")
        .execute(pool)
        .await
        .expect("null out log name");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bus_send_listen_is_point_to_point_across_a_group() {
    let (_db, _pool, bus) = bus().await;

    let total = 6usize;
    send_commands(&bus, total).await;

    let rec = Arc::new(Mutex::new(Vec::new()));
    let bus_a = bus.clone();
    let bus_b = bus.clone();
    let (ra, rb) = tokio::join!(
        bus_a.listen(
            recording_for(COMMAND_NAME, MessageKind::Command, rec.clone()),
            RunOptions::idempotent()
        ),
        bus_b.listen(
            recording_for(COMMAND_NAME, MessageKind::Command, rec.clone()),
            RunOptions::idempotent()
        ),
    );
    ra.expect("replica a drains");
    rb.expect("replica b drains");

    assert_eq!(
        recorded_ids(&rec),
        expected_ids("c", total),
        "every command handled exactly once across the group"
    );
}

#[tokio::test]
async fn bus_publish_subscribe_fans_out_across_groups() {
    let (_db, _pool, producer) = bus().await;
    let total = 4usize;
    publish_events(&producer, total).await;

    let expected = expected_ids("e", total);
    for group in ["projections", "audit"] {
        let bus = producer.clone().group(group);
        let rec = Arc::new(Mutex::new(Vec::new()));
        bus.subscribe(
            recording_for(EVENT_NAME, MessageKind::Event, rec.clone()),
            RunOptions::idempotent(),
        )
        .await
        .expect("subscriber drains");
        assert_eq!(
            recorded_ids(&rec),
            expected,
            "group {group} sees every event"
        );
    }
}

#[tokio::test]
async fn bus_subscribe_uses_named_service_as_consumer_group() {
    let (_db, pool, producer) = bus().await;
    publish_events(&producer, 3).await;

    let rec = Arc::new(Mutex::new(Vec::new()));
    SqliteBus::new(pool)
        .subscribe(
            named_recording_for(
                "order-projection",
                EVENT_NAME,
                MessageKind::Event,
                rec.clone(),
            ),
            RunOptions::idempotent(),
        )
        .await
        .expect("subscriber drains");

    assert_eq!(recorded_ids(&rec), expected_ids("e", 3));
}

#[tokio::test]
async fn retryable_command_failure_redelivers_then_completes() {
    let (_db, pool, bus) = bus().await;
    bus.send_message(command("c1")).await.expect("send command");

    let attempts = Arc::new(AtomicUsize::new(0));
    let seen = attempts.clone();
    let handlers = Arc::new(
        Handlers::new().on_command(COMMAND_NAME, move |_: &Message| {
            let seen = seen.clone();
            async move {
                let previous = seen.fetch_add(1, Ordering::SeqCst);
                if previous == 0 {
                    Err(TransportError::retryable("transient"))
                } else {
                    Ok(())
                }
            }
        }),
    );

    bus.listen(handlers, RunOptions::idempotent())
        .await
        .expect("listener drains after retry");

    assert_eq!(
        attempts.load(Ordering::SeqCst),
        2,
        "message was retried once after nack"
    );
    let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM bus_queue")
        .fetch_one(&pool)
        .await
        .expect("count queue");
    assert_eq!(remaining, 0, "retried command was acked and deleted");
}

#[tokio::test]
async fn retryable_event_failure_does_not_advance_offset() {
    let (_db, pool, bus) = bus().await;
    bus.publish_message(event("e1"))
        .await
        .expect("publish event");

    let attempts = Arc::new(AtomicUsize::new(0));
    let seen = attempts.clone();
    let handlers = Arc::new(Handlers::new().named("projections").on_event(
        EVENT_NAME,
        move |_: &Message| {
            let seen = seen.clone();
            async move {
                let previous = seen.fetch_add(1, Ordering::SeqCst);
                if previous == 0 {
                    Err(TransportError::retryable("transient"))
                } else {
                    Ok(())
                }
            }
        },
    ));

    SqliteBus::new(pool.clone())
        .subscribe(handlers, RunOptions::idempotent())
        .await
        .expect("subscriber drains after retry");

    assert_eq!(
        attempts.load(Ordering::SeqCst),
        2,
        "event was reread once because nack left the offset unmoved"
    );
    let offset: Option<i64> =
        sqlx::query_scalar("SELECT last_seq FROM bus_offset WHERE consumer = 'projections'")
            .fetch_optional(&pool)
            .await
            .expect("read offset");
    assert_eq!(offset, Some(1), "offset advanced only after success");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn busy_or_locked_writer_contention_is_retryable() {
    let db = TempDb::new();
    let setup_pool = db.pool().await;
    SqliteBus::new(setup_pool.clone())
        .ensure_tables()
        .await
        .expect("ensure tables");

    let lock_pool = db.pool_with_timeout(Duration::from_millis(0)).await;
    let send_pool = db.pool_with_timeout(Duration::from_millis(0)).await;
    let mut conn = lock_pool.acquire().await.expect("lock connection");
    sqlx::query("BEGIN EXCLUSIVE")
        .execute(&mut *conn)
        .await
        .expect("begin exclusive");

    let result = tokio::time::timeout(
        Duration::from_secs(2),
        SqliteBus::new(send_pool).send(COMMAND_NAME, PAYLOAD.to_vec()),
    )
    .await
    .expect("send should not hang behind busy lock");
    let err = result.expect_err("send should fail while database is locked");
    assert!(
        err.is_retryable(),
        "busy/locked contention must be retryable, got {err}"
    );

    sqlx::query("ROLLBACK")
        .execute(&mut *conn)
        .await
        .expect("rollback exclusive lock");
}

#[tokio::test]
async fn bus_listen_dead_letters_corrupt_queue_row_not_silently() {
    let (_db, pool, bus) = bus().await;
    recreate_nullable_queue_table(&pool).await;

    bus.send_message(command("poison"))
        .await
        .expect("send poison");
    corrupt_latest_queue_name(&pool).await;
    bus.send_message(command("ok")).await.expect("send ok");

    let rec = Arc::new(Mutex::new(Vec::new()));
    bus.listen(
        recording_for(COMMAND_NAME, MessageKind::Command, rec.clone()),
        RunOptions::idempotent(),
    )
    .await
    .expect("listen drains without surfacing corrupt row as fatal");

    assert_eq!(
        recorded_ids(&rec),
        vec!["ok".to_string()],
        "only the valid row handled"
    );
    let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM bus_queue")
        .fetch_one(&pool)
        .await
        .expect("count queue");
    assert_eq!(
        remaining, 0,
        "corrupt row routed through policy, not redelivered forever"
    );
}

#[tokio::test]
async fn bus_subscribe_dead_letters_corrupt_log_row_not_silently() {
    let (_db, pool, bus) = bus().await;
    recreate_nullable_log_table(&pool).await;

    bus.publish_message(event("poison"))
        .await
        .expect("publish leading poison");
    corrupt_latest_log_name(&pool).await;
    bus.publish_message(event("ok")).await.expect("publish ok");
    bus.publish_message(event("poison-tail"))
        .await
        .expect("publish trailing poison");
    corrupt_latest_log_name(&pool).await;

    let rec = Arc::new(Mutex::new(Vec::new()));
    SqliteBus::new(pool.clone())
        .group("projections")
        .subscribe(
            recording_for(EVENT_NAME, MessageKind::Event, rec.clone()),
            RunOptions::idempotent(),
        )
        .await
        .expect("subscribe drains past corrupt entries");

    assert_eq!(
        recorded_ids(&rec),
        vec!["ok".to_string()],
        "only the valid event handled"
    );
    let offset: Option<i64> =
        sqlx::query_scalar("SELECT last_seq FROM bus_offset WHERE consumer = 'projections'")
            .fetch_optional(&pool)
            .await
            .expect("read offset");
    let max_seq: i64 = sqlx::query_scalar("SELECT max(seq) FROM bus_log")
        .fetch_one(&pool)
        .await
        .expect("max seq");
    assert_eq!(
        offset,
        Some(max_seq),
        "offset advanced past corrupt entries through the failure policy"
    );
}
