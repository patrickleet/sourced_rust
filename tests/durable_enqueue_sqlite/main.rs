//! End-to-end durable-enqueue dispatch over a real SQL backend (in-memory
//! SQLite). Exercises `commit_outbox` (claim-in-transaction + immediate publish)
//! and the `with_bus` runtime against a persistent repository, not just the
//! in-memory `HashMapRepository` covered by the unit tests.

#![cfg(feature = "sqlite")]

use serde_json::{json, Value};

use distributed::bus::{Bus, InMemoryBus, RunOptions};
use distributed::microsvc::{Context, HandlerError, HasOutboxStore, Service, Session};
use distributed::{
    sourced, AggregateBuilder, AggregateRepository, AsyncOutboxStore, Entity, OutboxMessage,
    OutboxMessageStatus, QueuedRepository, Queueable, SqliteRepository,
};

#[derive(Default)]
struct Counter {
    entity: Entity,
    value: i64,
}

#[sourced(entity, aggregate_type = "counter")]
impl Counter {
    #[event("touched")]
    fn touch(&mut self, id: String) {
        self.entity.set_id(&id);
        self.value += 1;
    }
}

type Repo = AggregateRepository<QueuedRepository<SqliteRepository>, Counter>;

// Named fn (not a closure) so the higher-ranked `Handler` bound resolves.
async fn handle_touch(ctx: &Context<'_, Repo>) -> Result<Value, HandlerError> {
    let mut counter = Counter::default();
    counter.touch("c1".to_string())?;
    let message = OutboxMessage::create("evt-c1", "counter.touched", b"{}".to_vec())?;
    ctx.commit_outbox(&mut counter, message).await?;
    Ok(json!({ "value": counter.value }))
}

async fn service() -> Repo {
    SqliteRepository::connect_and_migrate("sqlite::memory:")
        .await
        .expect("sqlite repository should migrate")
        .queued()
        .aggregate::<Counter>()
}

#[tokio::test]
async fn commit_outbox_publishes_immediately_over_sqlite() {
    let microservice = Service::with_repo(service().await)
        .command("counter.touch")
        .handle(handle_touch)
        .with_bus(InMemoryBus::new());

    // The handler claims the outbox row in the SQL transaction, then publishes
    // it immediately through the attached bus.
    microservice
        .service()
        .dispatch("counter.touch", json!({}), Session::new())
        .await
        .unwrap();

    let store = microservice.service().repo().outbox_store();
    let published = store
        .messages_by_status_async(OutboxMessageStatus::Published)
        .await
        .unwrap();
    assert_eq!(published.len(), 1, "row should be published immediately");
    assert_eq!(published[0].id(), "evt-c1");
    assert!(
        store.pending_async().await.unwrap().is_empty(),
        "nothing should be left for the poller"
    );
}

#[tokio::test]
async fn run_consumes_command_and_publishes_over_sqlite() {
    let microservice = Service::with_repo(service().await)
        .command("counter.touch")
        .handle(handle_touch)
        .with_bus(InMemoryBus::new());

    // Enqueue a command, then run: listen is derived from the registered command,
    // drains it, and the handler publishes its outbox row through the bus.
    microservice
        .bus()
        .send("counter.touch", b"{}".to_vec())
        .await
        .unwrap();
    microservice.run(RunOptions::idempotent()).await.unwrap();

    let store = microservice.service().repo().outbox_store();
    let published = store
        .messages_by_status_async(OutboxMessageStatus::Published)
        .await
        .unwrap();
    assert_eq!(published.len(), 1);
    assert_eq!(published[0].id(), "evt-c1");
}
