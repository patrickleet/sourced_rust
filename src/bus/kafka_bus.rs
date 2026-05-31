//! Kafka [`Bus`] + [`BusConsumer`].
//!
//! Kafka shines as a partitioned, replayable log, and the point-to-point vs
//! fan-out distinction is entirely a **consumer-group** choice:
//!
//! - **`send` / `listen` (point-to-point, competing):** commands go to topics
//!   `{ns}.cmd.{name}`. `listen` joins a **shared** consumer group
//!   `{ns}.{group}.cmd`, so Kafka distributes the topic partitions across the
//!   group's members — each record is handled by exactly one replica.
//! - **`publish` / `subscribe` (fan-out):** events go to topics `{ns}.evt.{name}`.
//!   `subscribe` joins a group **per service** (`{ns}.{group}.evt`). Kafka
//!   delivers every record to every group, so each distinct `group` sees every
//!   event (replicas within a group still share its partitions → competing).
//!
//! The dispatched message name is the topic with its `{ns}.cmd.`/`{ns}.evt.`
//! prefix stripped. `{ns}` (namespace) scopes topics and groups so runs/apps
//! don't collide.
//!
//! Requires the `kafka` feature. Integration-tested in `tests/kafka_transport`.

use std::sync::Arc;
use std::time::Duration;

use super::kafka::{KafkaPublisher, KafkaSource};
use super::{
    run_source, AsyncMessagePublisher, Bus, BusConsumer, MessageRouter, RunOptions, TransportError,
};
use crate::microsvc::{Message, MessageKind};

const DEFAULT_FETCH_TIMEOUT: Duration = Duration::from_secs(8);

/// Kafka [`Bus`] + [`BusConsumer`]. Cheap to clone.
#[derive(Clone)]
pub struct KafkaBus {
    brokers: String,
    publisher: Arc<KafkaPublisher>,
    group: String,
    namespace: String,
    fetch_timeout: Duration,
}

impl KafkaBus {
    /// Connect a producer to `brokers` and build a bus. `group` is the consumer
    /// identity (same group ⇒ competing; different groups ⇒ fan-out); `namespace`
    /// scopes topics and group ids.
    pub async fn connect(
        brokers: &str,
        group: impl Into<String>,
        namespace: impl Into<String>,
    ) -> Result<Self, TransportError> {
        let publisher = KafkaPublisher::connect(brokers).await?;
        Ok(Self {
            brokers: brokers.to_string(),
            publisher: Arc::new(publisher),
            group: group.into(),
            namespace: namespace.into(),
            fetch_timeout: DEFAULT_FETCH_TIMEOUT,
        })
    }

    /// Override how long a `listen`/`subscribe` poll waits before idling. Kafka
    /// group bootstrap/rebalance takes time, so this is generous by default.
    pub fn with_fetch_timeout(mut self, timeout: Duration) -> Self {
        self.fetch_timeout = timeout;
        self
    }

    fn command_prefix(&self) -> String {
        format!("{}.cmd.", self.namespace)
    }

    fn event_prefix(&self) -> String {
        format!("{}.evt.", self.namespace)
    }

    async fn run<R: MessageRouter>(
        &self,
        router: Arc<R>,
        topics: Vec<String>,
        group_id: String,
        strip_prefix: String,
        options: RunOptions,
    ) -> Result<(), TransportError> {
        if topics.is_empty() {
            return Ok(());
        }
        let topic_refs: Vec<&str> = topics.iter().map(String::as_str).collect();
        let source = KafkaSource::connect(&self.brokers, &group_id, &topic_refs)
            .await?
            .with_fetch_timeout(self.fetch_timeout)
            .with_strip_prefix(strip_prefix);
        run_source(router, source, options).await
    }
}

impl Bus for KafkaBus {
    async fn send(&self, name: &str, payload: Vec<u8>) -> Result<(), TransportError> {
        self.send_message(Message::new(name, MessageKind::Command, payload))
            .await
    }

    async fn publish(&self, name: &str, payload: Vec<u8>) -> Result<(), TransportError> {
        self.publish_message(Message::new(name, MessageKind::Event, payload))
            .await
    }

    async fn send_message(&self, mut message: Message) -> Result<(), TransportError> {
        // The publisher uses the message name as the topic; namespace it.
        message.name = format!("{}{}", self.command_prefix(), message.name);
        self.publisher.publish(message).await
    }

    async fn publish_message(&self, mut message: Message) -> Result<(), TransportError> {
        message.name = format!("{}{}", self.event_prefix(), message.name);
        self.publisher.publish(message).await
    }
}

impl BusConsumer for KafkaBus {
    async fn listen<R: MessageRouter>(
        &self,
        router: Arc<R>,
        options: RunOptions,
    ) -> Result<(), TransportError> {
        let prefix = self.command_prefix();
        let topics: Vec<String> = router
            .subscription_plan()
            .commands
            .iter()
            .map(|name| format!("{prefix}{name}"))
            .collect();
        let group_id = format!("{}.{}.cmd", self.namespace, self.group);
        self.run(router, topics, group_id, prefix, options).await
    }

    async fn subscribe<R: MessageRouter>(
        &self,
        router: Arc<R>,
        options: RunOptions,
    ) -> Result<(), TransportError> {
        let prefix = self.event_prefix();
        let topics: Vec<String> = router
            .subscription_plan()
            .events
            .iter()
            .map(|name| format!("{prefix}{name}"))
            .collect();
        let group_id = format!("{}.{}.evt", self.namespace, self.group);
        self.run(router, topics, group_id, prefix, options).await
    }
}
