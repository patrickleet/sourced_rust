//! Bus integration for [`Service`]: `with_bus` (a builder step) and `run`.
//!
//! Attaching a bus does not change the service's type. `with_bus` is a plain
//! builder step on [`Service`] that (1) installs an outbox publisher on the
//! repository, so `repo.outbox(msg).commit(agg)` publishes immediately, and
//! (2) captures the bus's consume behavior as a type-erased closure on the
//! service. `run` drives that closure. There is no separate runtime type.

use std::future::{poll_fn, Future};
use std::pin::Pin;
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;

use super::dependencies::{ConfigurableOutboxPublisher, HasOutboxStore};
use super::Service;
use crate::bus::{Bus, BusConsumer, RunOptions, TransportError};
use crate::outbox::OutboxPublisherConfig;
use crate::outbox_worker::{BusOutboxPublishHook, BusPublisher};

/// Default lease for an immediate after-commit outbox publish. Short by design:
/// it only needs to cover commit → publish, so a crash before the publish
/// completes hands the row back to the polling worker quickly.
pub const DEFAULT_PUBLISH_LEASE: Duration = Duration::from_secs(5);

/// Default publish-failure ceiling before an outbox row is permanently failed.
pub const DEFAULT_MAX_PUBLISH_ATTEMPTS: u32 = 5;

impl<D> Service<D>
where
    D: Send + Sync + 'static + HasOutboxStore + ConfigurableOutboxPublisher,
{
    /// Attach a bus — a builder step that returns the same [`Service`].
    ///
    /// Two effects, both composing with the rest of the builder:
    /// - installs an outbox publisher on the repository, so
    ///   `repo.outbox(msg).commit(agg)` claims the row in the commit transaction
    ///   and publishes it immediately after commit through this bus (the polling
    ///   worker stays the crash/retry backstop);
    /// - captures how to consume, so [`run`](Self::run) listens for the
    ///   registered command names (competing) and subscribes to the event names
    ///   (fan-out).
    pub fn with_bus<B>(mut self, bus: B) -> Self
    where
        B: Bus + BusConsumer + 'static,
    {
        let bus = Arc::new(bus);

        let hook = BusOutboxPublishHook::new(
            self.dependencies().outbox_store(),
            BusPublisher::new(Arc::clone(&bus)),
            DEFAULT_MAX_PUBLISH_ATTEMPTS,
        );
        self.dependencies_mut()
            .configure_outbox_publisher(OutboxPublisherConfig::new(
                Arc::new(hook),
                format!("microsvc-immediate:{}", std::process::id()),
                DEFAULT_PUBLISH_LEASE,
            ));

        self.set_runner(Box::new(
            move |service: Arc<Service<D>>, options: RunOptions| {
                let bus = Arc::clone(&bus);
                Box::pin(async move { run_consumers(&*bus, service, options).await })
            },
        ));
        self
    }
}

impl<D: Send + Sync + 'static> Service<D> {
    /// Run against the bus attached with [`with_bus`](Self::with_bus): consume
    /// the registered command names (competing `listen`) and event names
    /// (fan-out `subscribe`) concurrently on the caller's runtime. Returns when
    /// the consumers stop (a pull source that drains, or the first error).
    ///
    /// Producing is handled on the commit path — `repo.outbox(msg).commit(agg)`
    /// publishes immediately once a bus is attached.
    ///
    /// # Panics
    /// If no bus was attached — call [`with_bus`](Self::with_bus) first.
    pub async fn run(mut self, options: RunOptions) -> Result<(), TransportError> {
        let runner = self
            .take_runner()
            .expect("Service::run requires a bus; call `with_bus` first");
        runner(Arc::new(self), options).await
    }
}

/// A running transport consumer (a `listen` or `subscribe` loop), borrowing the
/// bus for `'b`.
type ConsumerFuture<'b> = Pin<Box<dyn Future<Output = Result<(), TransportError>> + Send + 'b>>;

/// Drive a service's command/event consumers concurrently on the caller's
/// runtime — no spawn, no timer. Returns on the first error; finishes when all
/// consumers stop.
async fn run_consumers<'b, D, B>(
    bus: &'b B,
    service: Arc<Service<D>>,
    options: RunOptions,
) -> Result<(), TransportError>
where
    D: Send + Sync + 'static,
    B: Bus + BusConsumer,
{
    let plan = service.subscription_plan();
    let mut consumers: Vec<ConsumerFuture<'b>> = Vec::new();
    if !plan.commands.is_empty() {
        consumers.push(Box::pin(bus.listen(Arc::clone(&service), options.clone())));
    }
    if !plan.events.is_empty() {
        consumers.push(Box::pin(bus.subscribe(Arc::clone(&service), options)));
    }

    poll_fn(move |cx| {
        let mut index = 0;
        while index < consumers.len() {
            match consumers[index].as_mut().poll(cx) {
                Poll::Ready(Ok(())) => {
                    // Drop the finished consumer future; nothing left to poll.
                    let _finished = consumers.remove(index);
                }
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Pending => index += 1,
            }
        }
        if consumers.is_empty() {
            Poll::Ready(Ok(()))
        } else {
            Poll::Pending
        }
    })
    .await
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use crate::bus::{Bus, InMemoryBus, RunOptions};
    use crate::microsvc::{Context, HandlerError, HasOutboxStore, Service, Session};
    use crate::outbox_worker::OutboxStore;
    use crate::{
        sourced, AggregateBuilder, AggregateRepository, Entity, HashMapRepository, OutboxMessage,
        OutboxMessageStatus, Queueable, QueuedRepository, Snapshot,
    };

    #[derive(Default)]
    struct Dummy {
        entity: Entity,
    }

    #[sourced(entity)]
    impl Dummy {
        #[event("touched")]
        fn touch(&mut self) {
            if self.entity.id().is_empty() {
                self.entity.set_id("dummy-1");
            }
        }
    }

    #[tokio::test]
    async fn plain_commit_publishes_immediately_when_bus_is_attached() {
        let service = Service::new()
            .with_repo(HashMapRepository::new().queued().aggregate::<Dummy>())
            .with_bus(InMemoryBus::new());
        let store = service.repo().outbox_store();

        // The plain commit API publishes immediately because a bus is attached.
        let mut dummy = Dummy::default();
        dummy.touch().unwrap();
        let message = OutboxMessage::create("evt-1", "dummy.touched", b"{}".to_vec()).unwrap();
        let receipt = service
            .repo()
            .outbox(message)
            .commit(&mut dummy)
            .await
            .unwrap();
        assert_eq!(receipt.outbox_message_ids(), ["evt-1".to_string()]);

        let published = store
            .messages_by_status(OutboxMessageStatus::Published)
            .await
            .unwrap();
        assert_eq!(published.len(), 1, "row should be published at commit time");
        assert_eq!(published[0].id(), "evt-1");
        assert!(store.pending().await.unwrap().is_empty());
    }

    type TouchRepo = AggregateRepository<QueuedRepository<HashMapRepository>, Dummy>;

    // A named fn (not a closure) so the higher-ranked `Handler` bound resolves.
    async fn touch_and_publish(ctx: &Context<'_, TouchRepo>) -> Result<Value, HandlerError> {
        let mut dummy = Dummy::default();
        dummy.touch()?;
        let message = OutboxMessage::create("evt-1", "dummy.touched", b"{}".to_vec())?;
        // The good old API — commit publishes immediately because a bus is attached.
        ctx.repo().outbox(message).commit(&mut dummy).await?;
        Ok(json!({ "ok": true }))
    }

    #[tokio::test]
    async fn dispatch_through_a_handler_publishes_immediately() {
        let service = Service::new()
            .with_repo(HashMapRepository::new().queued().aggregate::<Dummy>())
            .command("dummy.touch")
            .handle(touch_and_publish)
            .with_bus(InMemoryBus::new());

        // The handler runs `outbox().commit()`: claim-in-transaction, then
        // immediate publish through the attached bus.
        service
            .dispatch("dummy.touch", json!({}), Session::new())
            .await
            .unwrap();

        let store = service.repo().outbox_store();
        let published = store
            .messages_by_status(OutboxMessageStatus::Published)
            .await
            .unwrap();
        assert_eq!(published.len(), 1, "row should be published immediately");
        assert_eq!(published[0].id(), "evt-1");
        assert!(store.pending().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn run_consumes_registered_commands_from_the_bus() {
        let bus = InMemoryBus::new();
        let service = Service::new()
            .with_repo(HashMapRepository::new().queued().aggregate::<Dummy>())
            .command("dummy.touch")
            .handle(touch_and_publish)
            .with_bus(bus.clone());
        // The store shares state with the repo, so it stays inspectable after
        // `run` consumes the service.
        let store = service.repo().outbox_store();

        // Enqueue a command on the bus, then run: `listen` is derived from the
        // registered command, drains the message, and the handler publishes.
        // `run` returns once the queue is empty (InMemoryBus yields `None`).
        bus.send("dummy.touch", b"{}".to_vec()).await.unwrap();
        service.run(RunOptions::idempotent()).await.unwrap();

        let published = store
            .messages_by_status(OutboxMessageStatus::Published)
            .await
            .unwrap();
        assert_eq!(
            published.len(),
            1,
            "run() should consume the command and publish its outbox row"
        );
    }

    #[derive(Default, Snapshot)]
    struct SnapCounter {
        entity: Entity,
        value: i64,
    }

    #[sourced(entity, aggregate_type = "snap_counter")]
    impl SnapCounter {
        #[event("touched")]
        fn touch(&mut self, id: String) {
            self.entity.set_id(&id);
            self.value += 1;
        }
    }

    // Snapshots are a transparent optimization: the repo type is unchanged.
    type SnapRepo = AggregateRepository<QueuedRepository<HashMapRepository>, SnapCounter>;

    async fn touch_snap(ctx: &Context<'_, SnapRepo>) -> Result<Value, HandlerError> {
        let mut counter = SnapCounter::default();
        counter.touch("s1".to_string())?;
        let message = OutboxMessage::create("evt-s1", "snap.touched", b"{}".to_vec())?;
        ctx.repo().outbox(message).commit(&mut counter).await?;
        Ok(json!({}))
    }

    #[tokio::test]
    async fn outbox_commit_publishes_with_snapshot_backed_repo() {
        // `outbox().commit()` must work for a snapshot-backed repository too: the
        // outbox row and the snapshot commit together in one transaction, then
        // the row publishes immediately.
        let service = Service::new()
            .with_repo(
                HashMapRepository::new()
                    .queued()
                    .aggregate::<SnapCounter>()
                    .with_snapshots(1),
            )
            .command("snap.touch")
            .handle(touch_snap)
            .with_bus(InMemoryBus::new());

        service
            .dispatch("snap.touch", json!({}), Session::new())
            .await
            .unwrap();

        let store = service.repo().outbox_store();
        let published = store
            .messages_by_status(OutboxMessageStatus::Published)
            .await
            .unwrap();
        assert_eq!(
            published.len(),
            1,
            "snapshot-backed outbox commit should publish immediately"
        );
        assert_eq!(published[0].id(), "evt-s1");
    }
}
