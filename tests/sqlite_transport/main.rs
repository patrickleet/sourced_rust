//! SQLite bus transport integration tests.
//!
//! Exercises `SqliteBus` over a local SQLite file: command work-queue claims,
//! event log offsets, retry/nack behavior, and corrupt-row handling.
#![cfg(feature = "sqlite")]

// Shared broker-test helpers (recording_for, message fixtures, bus scenarios).
#[path = "../transport_conformance/mod.rs"]
mod conformance;
use conformance::{command, event, recorded_ids, recording_for, COMMAND_NAME, EVENT_NAME, PAYLOAD};
#[path = "../support/sqlite.rs"]
mod sqlite_support;
use sqlite_support::TempDb;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use distributed::bus::{
    Bus, BusConsumer, Handlers, Message, MessageKind, RunOptions, SqliteBus, TransportError,
};
use sqlx::SqlitePool;
use tokio::sync::Notify;

async fn bus() -> (TempDb, SqlitePool, SqliteBus) {
    let db = TempDb::new("distributed_sqlite_bus_test");
    let pool = db.pool().await;
    let bus = SqliteBus::new(pool.clone()).group("orders");
    bus.ensure_tables().await.expect("ensure tables");
    (db, pool, bus)
}

/// Build a `SqliteBus` over `pool` for `group` (empty `group` = no group).
/// Tables are already ensured by [`bus`].
fn sqlite_bus(pool: &SqlitePool, group: &str) -> SqliteBus {
    let bus = SqliteBus::new(pool.clone());
    if group.is_empty() {
        bus
    } else {
        bus.group(group)
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
            claim_token  TEXT,
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
            content_type TEXT DEFAULT 'application/json',
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

async fn corrupt_latest_queue_kind(pool: &SqlitePool) {
    sqlx::query("UPDATE bus_queue SET kind = 'bogus' WHERE seq = (SELECT max(seq) FROM bus_queue)")
        .execute(pool)
        .await
        .expect("corrupt queue kind");
}

async fn corrupt_latest_log_name(pool: &SqlitePool) {
    sqlx::query("UPDATE bus_log SET name = NULL WHERE seq = (SELECT max(seq) FROM bus_log)")
        .execute(pool)
        .await
        .expect("null out log name");
}

async fn corrupt_latest_log_kind(pool: &SqlitePool) {
    sqlx::query("UPDATE bus_log SET kind = 'bogus' WHERE seq = (SELECT max(seq) FROM bus_log)")
        .execute(pool)
        .await
        .expect("corrupt log kind");
}

async fn corrupt_latest_log_metadata(pool: &SqlitePool) {
    sqlx::query(
        "UPDATE bus_log SET metadata = 'not-json' WHERE seq = (SELECT max(seq) FROM bus_log)",
    )
    .execute(pool)
    .await
    .expect("corrupt log metadata");
}

async fn corrupt_latest_log_content_type(pool: &SqlitePool) {
    sqlx::query(
        "UPDATE bus_log SET content_type = NULL WHERE seq = (SELECT max(seq) FROM bus_log)",
    )
    .execute(pool)
    .await
    .expect("corrupt log content type");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bus_send_listen_is_point_to_point_across_a_group() {
    let (_db, pool, _bus) = bus().await;
    conformance::bus_send_listen_is_point_to_point_across_a_group(|group| {
        let bus = sqlite_bus(&pool, group);
        async move { bus }
    })
    .await;
}

#[tokio::test]
async fn bus_publish_subscribe_fans_out_across_groups() {
    let (_db, pool, _bus) = bus().await;
    conformance::bus_publish_subscribe_fans_out_across_groups(|group| {
        let bus = sqlite_bus(&pool, group);
        async move { bus }
    })
    .await;
}

#[tokio::test]
async fn bus_subscribe_uses_named_service_as_consumer_group() {
    let (_db, pool, _bus) = bus().await;
    conformance::bus_subscribe_uses_named_service_as_consumer_group(|| {
        let bus = sqlite_bus(&pool, "");
        async move { bus }
    })
    .await;
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

#[tokio::test]
async fn bus_schema_rejects_unsupported_message_kind() {
    let (_db, pool, _bus) = bus().await;

    let queue_err = sqlx::query("INSERT INTO bus_queue (name, kind, payload) VALUES (?, ?, ?)")
        .bind(COMMAND_NAME)
        .bind("bogus")
        .bind(PAYLOAD.to_vec())
        .execute(&pool)
        .await
        .expect_err("queue kind check rejects unsupported message kind");
    assert!(
        queue_err.to_string().contains("CHECK"),
        "unexpected queue kind error: {queue_err}"
    );

    let log_err = sqlx::query("INSERT INTO bus_log (name, kind, payload) VALUES (?, ?, ?)")
        .bind(EVENT_NAME)
        .bind("bogus")
        .bind(PAYLOAD.to_vec())
        .execute(&pool)
        .await
        .expect_err("log kind check rejects unsupported message kind");
    assert!(
        log_err.to_string().contains("CHECK"),
        "unexpected log kind error: {log_err}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn busy_or_locked_writer_contention_is_retryable() {
    let db = TempDb::new("distributed_sqlite_bus_test");
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
    bus.send_message(command("poison-kind"))
        .await
        .expect("send poison kind");
    corrupt_latest_queue_kind(&pool).await;
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
    bus.publish_message(event("poison-kind"))
        .await
        .expect("publish corrupt kind");
    corrupt_latest_log_kind(&pool).await;
    bus.publish_message(event("poison-metadata"))
        .await
        .expect("publish corrupt metadata");
    corrupt_latest_log_metadata(&pool).await;
    bus.publish_message(event("poison-content-type"))
        .await
        .expect("publish corrupt content type");
    corrupt_latest_log_content_type(&pool).await;

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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn expired_queue_claim_cannot_be_settled_by_stale_worker() {
    let (_db, pool, bus) = bus().await;
    let bus = bus.with_lease(Duration::from_millis(250));
    bus.send_message(command("c1")).await.expect("send command");

    let attempts = Arc::new(AtomicUsize::new(0));
    let first_claimed = Arc::new(Notify::new());
    let second_claimed = Arc::new(Notify::new());
    let allow_second_finish = Arc::new(Notify::new());

    let handlers = Arc::new({
        let attempts = attempts.clone();
        let first_claimed = first_claimed.clone();
        let second_claimed = second_claimed.clone();
        let allow_second_finish = allow_second_finish.clone();
        Handlers::new().on_command(COMMAND_NAME, move |_: &Message| {
            let attempt = attempts.fetch_add(1, Ordering::SeqCst);
            let first_claimed = first_claimed.clone();
            let second_claimed = second_claimed.clone();
            let allow_second_finish = allow_second_finish.clone();
            async move {
                match attempt {
                    0 => {
                        first_claimed.notify_one();
                        tokio::time::sleep(Duration::from_millis(420)).await;
                        Ok(())
                    }
                    1 => {
                        second_claimed.notify_one();
                        allow_second_finish.notified().await;
                        Err(TransportError::retryable("second claim releases for retry"))
                    }
                    _ => Ok(()),
                }
            }
        })
    });

    let first = tokio::spawn({
        let bus = bus.clone();
        let handlers = handlers.clone();
        async move { bus.listen(handlers, RunOptions::idempotent()).await }
    });
    tokio::time::timeout(Duration::from_secs(2), first_claimed.notified())
        .await
        .expect("first worker claimed the command");

    tokio::time::sleep(Duration::from_millis(300)).await;
    let second = tokio::spawn({
        let bus = bus.clone();
        let handlers = handlers.clone();
        async move { bus.listen(handlers, RunOptions::idempotent()).await }
    });
    tokio::time::timeout(Duration::from_secs(2), second_claimed.notified())
        .await
        .expect("second worker reclaimed the expired lease");

    tokio::time::timeout(Duration::from_secs(2), first)
        .await
        .expect("stale first worker finished")
        .expect("first worker joined")
        .expect("first listener drains");

    allow_second_finish.notify_waiters();
    tokio::time::timeout(Duration::from_secs(2), second)
        .await
        .expect("second worker finished")
        .expect("second worker joined")
        .expect("second listener drains");

    assert_eq!(
        attempts.load(Ordering::SeqCst),
        3,
        "stale ack did not delete the newer claim before it could be retried"
    );
    let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM bus_queue")
        .fetch_one(&pool)
        .await
        .expect("count queue");
    assert_eq!(remaining, 0, "retried command was eventually acked");
}
