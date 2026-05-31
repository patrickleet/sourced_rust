//! RabbitMQ (AMQP 0-9-1) [`Bus`] + [`BusConsumer`].
//!
//! RabbitMQ shines through its exchange topologies, so the two bus surfaces map
//! onto two different exchange shapes:
//!
//! - **`send` / `listen` (point-to-point, competing):** the **default exchange**
//!   routes a message to the queue named by its routing key. `send(name)`
//!   declares a durable queue `{ns}.cmd.{name}` and publishes to it; `listen`
//!   consumes those queues. Replicas sharing a queue compete (AMQP round-robins
//!   across consumers) — point-to-point.
//! - **`publish` / `subscribe` (fan-out):** a durable **topic exchange**
//!   `{ns}.events`. `publish(name)` publishes to it with routing key `name`; each
//!   `subscribe`r (keyed by `group`) declares its **own** queue `{ns}.evt.{group}`
//!   bound to the exchange for its event names. Distinct groups get distinct
//!   queues, so every group receives every event — fan-out (replicas within a
//!   group still compete on that group's queue).
//!
//! The `group` is the logical consumer identity. `{ns}` (namespace) scopes queue
//! and exchange names so multiple apps can share a broker.
//!
//! Requires the `rabbitmq` feature. Integration-tested in `tests/rabbitmq_transport`.

use std::sync::Arc;

use lapin::options::{
    BasicGetOptions, BasicPublishOptions, ConfirmSelectOptions, ExchangeDeclareOptions,
    QueueBindOptions, QueueDeclareOptions,
};
use lapin::types::FieldTable;
use lapin::{Channel, ExchangeKind};

use super::rabbitmq::{connect_channel, message_properties, RabbitReceived};
use super::source::AsyncMessageSource;
use super::{run_source, Bus, BusConsumer, MessageRouter, RunOptions, TransportError};
use super::{Message, MessageKind};

fn retryable(context: &str, err: impl std::fmt::Display) -> TransportError {
    TransportError::retryable(format!("{context}: {err}"))
}

/// RabbitMQ [`Bus`] + [`BusConsumer`].
pub struct RabbitBus {
    uri: String,
    channel: Channel,
    group: String,
    namespace: String,
    events_exchange: String,
}

impl RabbitBus {
    /// Connect to an AMQP URI and build a bus. `group` is the consumer identity
    /// (same group ⇒ competing; different groups ⇒ fan-out); `namespace` scopes
    /// queue/exchange names.
    pub async fn connect(
        uri: &str,
        group: impl Into<String>,
        namespace: impl Into<String>,
    ) -> Result<Self, TransportError> {
        let channel = connect_channel(uri).await?;
        channel
            .confirm_select(ConfirmSelectOptions::default())
            .await
            .map_err(|err| retryable("amqp confirm_select", err))?;
        let namespace = namespace.into();
        Ok(Self {
            uri: uri.to_string(),
            channel,
            group: group.into(),
            events_exchange: format!("{namespace}.events"),
            namespace,
        })
    }

    fn command_queue(&self, name: &str) -> String {
        format!("{}.cmd.{name}", self.namespace)
    }

    fn command_prefix(&self) -> String {
        format!("{}.cmd.", self.namespace)
    }

    fn group_queue(&self) -> String {
        format!("{}.evt.{}", self.namespace, self.group)
    }

    async fn declare_queue(&self, channel: &Channel, queue: &str) -> Result<(), TransportError> {
        channel
            .queue_declare(
                queue,
                QueueDeclareOptions {
                    durable: true,
                    ..Default::default()
                },
                FieldTable::default(),
            )
            .await
            .map_err(|err| retryable("amqp queue_declare", err))?;
        Ok(())
    }

    async fn declare_events_exchange(&self, channel: &Channel) -> Result<(), TransportError> {
        channel
            .exchange_declare(
                &self.events_exchange,
                ExchangeKind::Topic,
                ExchangeDeclareOptions {
                    durable: true,
                    ..Default::default()
                },
                FieldTable::default(),
            )
            .await
            .map_err(|err| retryable("amqp exchange_declare", err))?;
        Ok(())
    }

    async fn publish_confirmed(
        &self,
        exchange: &str,
        routing_key: &str,
        message: &Message,
    ) -> Result<(), TransportError> {
        let confirm = self
            .channel
            .basic_publish(
                exchange,
                routing_key,
                BasicPublishOptions::default(),
                &message.payload,
                message_properties(message),
            )
            .await
            .map_err(|err| retryable("amqp publish", err))?;
        if confirm
            .await
            .map_err(|err| retryable("amqp publisher confirm", err))?
            .is_nack()
        {
            return Err(TransportError::retryable("amqp publisher confirm: nack"));
        }
        Ok(())
    }

    /// Declare the topic exchange, this group's queue, and bind it to the
    /// service's event names — the durable setup `subscribe` needs. Exposed so a
    /// producer can ensure all subscriber bindings exist *before* publishing
    /// (RabbitMQ drops events with no matching binding).
    pub async fn ensure_subscription<R: MessageRouter>(
        &self,
        router: &R,
    ) -> Result<(), TransportError> {
        self.declare_events_exchange(&self.channel).await?;
        let queue = self.group_queue();
        self.declare_queue(&self.channel, &queue).await?;
        let plan = router.subscription_plan();
        for name in &plan.events {
            self.channel
                .queue_bind(
                    &queue,
                    &self.events_exchange,
                    name,
                    QueueBindOptions::default(),
                    FieldTable::default(),
                )
                .await
                .map_err(|err| retryable("amqp queue_bind", err))?;
        }
        Ok(())
    }
}

impl Bus for RabbitBus {
    async fn send(&self, name: &str, payload: Vec<u8>) -> Result<(), TransportError> {
        self.send_message(Message::new(name, MessageKind::Command, payload))
            .await
    }

    async fn publish(&self, name: &str, payload: Vec<u8>) -> Result<(), TransportError> {
        self.publish_message(Message::new(name, MessageKind::Event, payload))
            .await
    }

    async fn send_message(&self, mut message: Message) -> Result<(), TransportError> {
        // Default exchange routes by routing key == queue name; declare the queue
        // so the command is retained until a listener consumes it.
        let queue = self.command_queue(message.name());
        self.declare_queue(&self.channel, &queue).await?;
        message.name = queue.clone();
        self.publish_confirmed("", &queue, &message).await
    }

    async fn publish_message(&self, message: Message) -> Result<(), TransportError> {
        self.declare_events_exchange(&self.channel).await?;
        let routing_key = message.name().to_string();
        self.publish_confirmed(&self.events_exchange, &routing_key, &message)
            .await
    }
}

impl BusConsumer for RabbitBus {
    async fn listen<R: MessageRouter>(
        &self,
        router: Arc<R>,
        options: RunOptions,
    ) -> Result<(), TransportError> {
        let channel = connect_channel(&self.uri).await?;
        let plan = router.subscription_plan();
        let mut queues = Vec::new();
        for name in &plan.commands {
            let queue = self.command_queue(name);
            self.declare_queue(&channel, &queue).await?;
            queues.push(queue);
        }
        if queues.is_empty() {
            return Ok(());
        }
        let source = RabbitBusSource {
            channel,
            queues,
            strip_prefix: Some(self.command_prefix()),
        };
        run_source(router, source, options).await
    }

    async fn subscribe<R: MessageRouter>(
        &self,
        router: Arc<R>,
        options: RunOptions,
    ) -> Result<(), TransportError> {
        self.ensure_subscription(router.as_ref()).await?;
        if router.subscription_plan().events.is_empty() {
            return Ok(());
        }
        let channel = connect_channel(&self.uri).await?;
        let source = RabbitBusSource {
            channel,
            queues: vec![self.group_queue()],
            // Events are published with routing key == the bare event name.
            strip_prefix: None,
        };
        run_source(router, source, options).await
    }
}

/// Polls one or more queues with `basic_get`, resolving the message name from the
/// delivery's routing key (stripping `strip_prefix` for command queues).
struct RabbitBusSource {
    channel: Channel,
    queues: Vec<String>,
    strip_prefix: Option<String>,
}

impl AsyncMessageSource for RabbitBusSource {
    type Received = RabbitReceived;

    async fn recv(&mut self) -> Result<Option<Self::Received>, TransportError> {
        for queue in &self.queues {
            let got = self
                .channel
                .basic_get(queue, BasicGetOptions::default())
                .await
                .map_err(|err| retryable("amqp basic_get", err))?;
            if let Some(get) = got {
                let routing_key = get.delivery.routing_key.to_string();
                let name = match &self.strip_prefix {
                    Some(prefix) => routing_key
                        .strip_prefix(prefix.as_str())
                        .unwrap_or(&routing_key)
                        .to_string(),
                    None => routing_key,
                };
                return Ok(Some(RabbitReceived::from_delivery_with_name(
                    get.delivery,
                    name,
                )));
            }
        }
        Ok(None)
    }
}
