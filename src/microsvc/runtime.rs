//! Runtime that ties a [`Service`] to a bus.
//!
//! `register_handlers!` builds a [`Service`] that is purely a consumer. Attaching
//! a bus with [`Service::with_bus`] turns it into a [`Microservice`] that carries
//! the transport config for both sides: it can drain committed outbox rows to the
//! bus (produce) and — once `run` lands — derive listen/subscribe from the
//! registered handlers (consume).
//!
//! The producing side is a thin assembly over [`OutboxDispatcher`] and
//! [`BusPublisher`]: [`Microservice::dispatcher`] hands back a dispatcher whose
//! store is the service's own outbox store and whose publisher routes through the
//! attached bus by [`MessageKind`](crate::bus::MessageKind). The same dispatcher
//! backs immediate after-commit dispatch and a background poll loop.

use std::sync::Arc;
use std::time::Duration;

use super::dependencies::{HasOutboxStore, HasRepo};
use super::service::ImmediatePublish;
use super::Service;
use crate::bus::{Bus, BusConsumer, DynPublisher, RunOptions, TransportError};
use crate::outbox_worker::{BusPublisher, OutboxDispatcher};

/// Default lease for an immediate after-commit outbox publish.
///
/// Short by design: it only needs to cover commit → publish, so a crash before
/// the publish completes hands the row back to the polling worker quickly.
pub const DEFAULT_PUBLISH_LEASE: Duration = Duration::from_secs(5);

/// Default publish-failure ceiling before an outbox row is permanently failed.
pub const DEFAULT_MAX_PUBLISH_ATTEMPTS: u32 = 5;

/// A [`Service`] bound to a bus.
///
/// Holds the service and bus behind `Arc`s so the produce side (the dispatcher)
/// and the consume side (listen/subscribe) can share them.
pub struct Microservice<D, B> {
    service: Arc<Service<D>>,
    bus: Arc<B>,
    worker_id: String,
    publish_lease: Duration,
    max_attempts: u32,
}

impl<D, B> Microservice<D, B> {
    /// Bind a service to a bus with default dispatch settings.
    pub fn new(service: Arc<Service<D>>, bus: Arc<B>) -> Self {
        Self {
            service,
            bus,
            worker_id: format!("microsvc-immediate:{}", std::process::id()),
            publish_lease: DEFAULT_PUBLISH_LEASE,
            max_attempts: DEFAULT_MAX_PUBLISH_ATTEMPTS,
        }
    }

    /// The bound service.
    pub fn service(&self) -> &Arc<Service<D>> {
        &self.service
    }

    /// The bound bus.
    pub fn bus(&self) -> &Arc<B> {
        &self.bus
    }

    /// Set the worker id used to scope outbox claims (default
    /// `microsvc-immediate:<pid>`).
    pub fn with_worker_id(mut self, worker_id: impl Into<String>) -> Self {
        self.worker_id = worker_id.into();
        self
    }

    /// Set the lease taken when claiming an outbox row for publication.
    pub fn with_publish_lease(mut self, lease: Duration) -> Self {
        self.publish_lease = lease;
        self
    }

    /// Set the publish-failure ceiling before a row is permanently failed.
    pub fn with_max_attempts(mut self, max_attempts: u32) -> Self {
        self.max_attempts = max_attempts;
        self
    }
}

impl<D, B> Microservice<D, B>
where
    D: HasRepo + Send + Sync + 'static,
    D::Repo: HasOutboxStore,
    B: Bus,
{
    /// Build a dispatcher that drains committed outbox rows to the bus.
    ///
    /// The store is the service's own outbox store; the publisher routes each
    /// message to the bus by kind (commands point-to-point, events fan-out). The
    /// same dispatcher is used by immediate after-commit dispatch
    /// (`dispatch_ids`) and a background poll loop (`dispatch_batch`).
    pub fn dispatcher(
        &self,
    ) -> OutboxDispatcher<<D::Repo as HasOutboxStore>::OutboxStore, BusPublisher<B>> {
        OutboxDispatcher::new(
            self.service.repo().outbox_store(),
            BusPublisher::new(Arc::clone(&self.bus)),
            self.worker_id.clone(),
            self.publish_lease,
            self.max_attempts,
        )
    }
}

impl<D, B> Microservice<D, B>
where
    D: Send + Sync + 'static,
    B: Bus + BusConsumer,
{
    /// Run the service against the attached bus.
    ///
    /// Derives the consumers from the registered handlers: command handlers are
    /// consumed with competing (point-to-point) `listen`, event handlers with
    /// fan-out `subscribe`. Both run concurrently on the caller's runtime;
    /// `run` returns when the consumers stop (a pull source that drains, or the
    /// first error).
    ///
    /// Producing is handled separately: the primary path is immediate publish via
    /// [`Context::commit_outbox`](crate::microsvc::Context::commit_outbox); the
    /// background poll loop (the crash backstop, which needs an async timer) is
    /// driven from [`Self::dispatcher`] by a runtime that provides one.
    pub async fn run(&self, options: RunOptions) -> Result<(), TransportError> {
        use std::future::{poll_fn, Future};
        use std::pin::Pin;
        use std::task::Poll;

        let plan = self.service.subscription_plan();
        let mut consumers: Vec<
            Pin<Box<dyn Future<Output = Result<(), TransportError>> + Send + '_>>,
        > = Vec::new();
        if !plan.commands.is_empty() {
            consumers.push(Box::pin(
                self.bus.listen(Arc::clone(&self.service), options.clone()),
            ));
        }
        if !plan.events.is_empty() {
            consumers.push(Box::pin(
                self.bus.subscribe(Arc::clone(&self.service), options),
            ));
        }

        // Drive every consumer concurrently on the caller's runtime — no spawn,
        // no timer. Return on the first error; finish when all consumers stop.
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
}

impl<D: Send + Sync + 'static> Service<D> {
    /// Attach a bus, producing a [`Microservice`] that carries the transport
    /// config for both producing (outbox dispatch) and consuming
    /// (listen/subscribe).
    ///
    /// Attaching a bus also enables immediate after-commit publish: handlers
    /// that call [`Context::commit_outbox`](crate::microsvc::Context::commit_outbox)
    /// claim the outbox row in the commit transaction and publish it through this
    /// bus. The immediate path uses [`DEFAULT_PUBLISH_LEASE`] and
    /// [`DEFAULT_MAX_PUBLISH_ATTEMPTS`]; the `with_publish_lease` /
    /// `with_max_attempts` setters configure the background poll loop.
    pub fn with_bus<B>(mut self, bus: B) -> Microservice<D, B>
    where
        B: Bus + 'static,
    {
        let bus = Arc::new(bus);
        let publisher: Arc<dyn DynPublisher> = Arc::new(BusPublisher::new(Arc::clone(&bus)));
        self.set_immediate_publish(ImmediatePublish {
            publisher,
            worker_id: format!("microsvc-immediate:{}", std::process::id()),
            lease: DEFAULT_PUBLISH_LEASE,
            max_attempts: DEFAULT_MAX_PUBLISH_ATTEMPTS,
        });
        Microservice::new(Arc::new(self), bus)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use crate::bus::{Bus, InMemoryBus, RunOptions};
    use crate::microsvc::{Context, HandlerError, HasOutboxStore, Service, Session};
    use crate::outbox_worker::AsyncOutboxStore;
    use crate::{
        sourced, AggregateBuilder, AggregateRepository, Entity, HashMapRepository, OutboxMessage,
        OutboxMessageStatus, QueuedRepository, Queueable,
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
    async fn dispatcher_drains_committed_outbox_row_to_the_bus() {
        let service =
            Service::with_repo(HashMapRepository::new().queued().aggregate::<Dummy>());

        let microservice = service.with_bus(InMemoryBus::new());

        // Commit an aggregate + outbox row through the bound service's repo.
        let mut dummy = Dummy::default();
        dummy.touch().unwrap();
        let message = OutboxMessage::create("evt-1", "dummy.touched", b"{}".to_vec()).unwrap();
        let receipt = microservice
            .service()
            .repo()
            .outbox(message)
            .commit(&mut dummy)
            .await
            .unwrap();
        assert_eq!(receipt.outbox_message_ids(), ["evt-1".to_string()]);

        // The dispatcher (store + bus) drains the committed row to the bus.
        let outcome = microservice.dispatcher().dispatch_batch(10).await.unwrap();
        assert_eq!(outcome.published, 1);
        assert_eq!(outcome.released, 0);
        assert_eq!(outcome.failed, 0);
    }

    type TouchRepo = AggregateRepository<QueuedRepository<HashMapRepository>, Dummy>;

    // A named fn (not a closure) so the higher-ranked `Handler` bound resolves.
    async fn touch_and_publish(ctx: &Context<'_, TouchRepo>) -> Result<Value, HandlerError> {
        let mut dummy = Dummy::default();
        dummy.touch()?;
        let message = OutboxMessage::create("evt-1", "dummy.touched", b"{}".to_vec())?;
        ctx.commit_outbox(&mut dummy, message).await?;
        Ok(json!({ "ok": true }))
    }

    #[tokio::test]
    async fn commit_outbox_publishes_immediately_when_bus_is_attached() {
        let service = Service::with_repo(HashMapRepository::new().queued().aggregate::<Dummy>())
            .command("dummy.touch")
            .handle(touch_and_publish);
        let microservice = service.with_bus(InMemoryBus::new());

        // Dispatching the command runs the handler, which calls `commit_outbox`:
        // claim-in-transaction, then immediate publish through the attached bus.
        microservice
            .service()
            .dispatch("dummy.touch", json!({}), Session::new())
            .await
            .unwrap();

        // The row was published immediately (claim-in-tx -> publish -> complete),
        // so nothing is left pending for the poller.
        let store = microservice.service().repo().outbox_store();
        let published = store
            .messages_by_status_async(OutboxMessageStatus::Published)
            .await
            .unwrap();
        assert_eq!(published.len(), 1, "row should be published immediately");
        assert_eq!(published[0].id(), "evt-1");
        assert!(
            store.pending_async().await.unwrap().is_empty(),
            "no row should be left for the poller"
        );
    }

    #[tokio::test]
    async fn run_consumes_registered_commands_from_the_bus() {
        let service = Service::with_repo(HashMapRepository::new().queued().aggregate::<Dummy>())
            .command("dummy.touch")
            .handle(touch_and_publish);
        let microservice = service.with_bus(InMemoryBus::new());

        // Enqueue a command on the bus, then run: `listen` is derived from the
        // registered command, drains the message, and the handler runs
        // (commit_outbox publishes immediately). `run` returns once the queue is
        // empty (InMemoryBus source yields `None`).
        microservice
            .bus()
            .send("dummy.touch", b"{}".to_vec())
            .await
            .unwrap();
        microservice.run(RunOptions::idempotent()).await.unwrap();

        let store = microservice.service().repo().outbox_store();
        let published = store
            .messages_by_status_async(OutboxMessageStatus::Published)
            .await
            .unwrap();
        assert_eq!(
            published.len(),
            1,
            "run() should consume the command and publish its outbox row"
        );
    }
}
