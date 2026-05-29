//! RabbitMQ (AMQP 0-9-1) transport adapter.
//!
//! [`RabbitPublisher`] publishes a canonical [`Message`] to the default exchange
//! keyed by the message name, waiting for the **publisher confirm** (the durable
//! publish threshold). [`RabbitSource`] polls a queue with `basic_get` and
//! settles via AMQP ack/nack/reject (ack→ack, nack→nack+requeue,
//! dead-letter/park→reject without requeue, which routes to a dead-letter
//! exchange when one is configured on the queue).
//!
//! Requires the `rabbitmq` feature. Integration-tested in
//! `tests/rabbitmq_transport` against a broker (see `compose.yaml`).

use lapin::message::Delivery;
use lapin::options::{
    BasicAckOptions, BasicGetOptions, BasicNackOptions, BasicPublishOptions, BasicRejectOptions,
    ConfirmSelectOptions, QueueDeclareOptions,
};
use lapin::types::{AMQPValue, FieldTable, ShortString};
use lapin::{BasicProperties, Channel, Connection, ConnectionProperties};

use super::source::{AsyncMessageSource, ReceivedMessage};
use super::{AsyncMessagePublisher, TransportError};
use crate::microsvc::{Message, MessageKind};

const MESSAGE_KIND_HEADER: &str = "x-sourced-kind";

fn retryable(context: &str, err: impl std::fmt::Display) -> TransportError {
    TransportError::retryable(format!("{context}: {err}"))
}

async fn connect_channel(uri: &str) -> Result<Channel, TransportError> {
    let connection = Connection::connect(uri, ConnectionProperties::default())
        .await
        .map_err(|err| retryable("amqp connect", err))?;
    connection
        .create_channel()
        .await
        .map_err(|err| retryable("amqp channel", err))
}

/// Publishes canonical messages to the default exchange, keyed by message name.
pub struct RabbitPublisher {
    channel: Channel,
}

impl RabbitPublisher {
    /// Wrap an existing channel (publisher confirms are enabled on connect).
    pub fn new(channel: Channel) -> Self {
        Self { channel }
    }

    /// Connect to an AMQP URI and enable publisher confirms.
    pub async fn connect(uri: &str) -> Result<Self, TransportError> {
        let channel = connect_channel(uri).await?;
        channel
            .confirm_select(ConfirmSelectOptions::default())
            .await
            .map_err(|err| retryable("amqp confirm_select", err))?;
        Ok(Self::new(channel))
    }
}

fn message_properties(message: &Message) -> BasicProperties {
    let mut headers = FieldTable::default();
    headers.insert(
        ShortString::from(MESSAGE_KIND_HEADER),
        AMQPValue::LongString(kind_str(message.kind).into()),
    );
    for (key, value) in &message.metadata {
        headers.insert(
            ShortString::from(key.as_str()),
            AMQPValue::LongString(value.as_str().into()),
        );
    }
    let mut properties = BasicProperties::default().with_headers(headers);
    if let Some(id) = message.id() {
        properties = properties.with_message_id(ShortString::from(id));
    }
    properties
}

impl AsyncMessagePublisher for RabbitPublisher {
    async fn publish(&self, message: Message) -> Result<(), TransportError> {
        let confirm = self
            .channel
            .basic_publish(
                "", // default exchange: routes to the queue named by the routing key
                message.name(),
                BasicPublishOptions::default(),
                &message.payload,
                message_properties(&message),
            )
            .await
            .map_err(|err| retryable("amqp publish", err))?;
        let confirmation = confirm
            .await
            .map_err(|err| retryable("amqp publisher confirm", err))?;
        if confirmation.is_nack() {
            return Err(TransportError::retryable("amqp publisher confirm: nack"));
        }
        Ok(())
    }
}

/// Polls a single queue with `basic_get`.
pub struct RabbitSource {
    channel: Channel,
    queue: String,
}

impl RabbitSource {
    /// Wrap an existing channel bound to `queue`.
    pub fn new(channel: Channel, queue: impl Into<String>) -> Self {
        Self {
            channel,
            queue: queue.into(),
        }
    }

    /// Connect, declare a durable queue, and poll it. The default exchange routes
    /// a message published with routing key == `queue` into this queue, so the
    /// queue name is the message name consumers subscribe to.
    pub async fn connect(uri: &str, queue: &str) -> Result<Self, TransportError> {
        let channel = connect_channel(uri).await?;
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
        Ok(Self::new(channel, queue))
    }
}

impl AsyncMessageSource for RabbitSource {
    type Received = RabbitReceived;

    async fn recv(&mut self) -> Result<Option<Self::Received>, TransportError> {
        let message = self
            .channel
            .basic_get(&self.queue, BasicGetOptions::default())
            .await
            .map_err(|err| retryable("amqp basic_get", err))?;
        Ok(message.map(|get| RabbitReceived::from_delivery(get.delivery, self.queue.clone())))
    }
}

/// An AMQP delivery plus its settle handle.
pub struct RabbitReceived {
    delivery: Delivery,
    message: Message,
}

impl RabbitReceived {
    fn from_delivery(delivery: Delivery, queue: String) -> Self {
        let payload = delivery.data.clone();
        let id = delivery
            .properties
            .message_id()
            .as_ref()
            .map(|s| s.to_string());
        let mut kind = MessageKind::Event;
        let mut metadata = Vec::new();
        if let Some(headers) = delivery.properties.headers().as_ref() {
            for (key, value) in headers.inner() {
                let key = key.to_string();
                let value = amqp_value_to_string(value);
                if key == MESSAGE_KIND_HEADER {
                    kind = kind_from_str(&value);
                } else {
                    metadata.push((key, value));
                }
            }
        }
        // The queue name is the routed message name.
        let mut message = Message::new(queue, kind, payload);
        message.id = id;
        message.metadata = metadata;
        Self { delivery, message }
    }
}

impl ReceivedMessage for RabbitReceived {
    fn message(&self) -> &Message {
        &self.message
    }

    async fn ack(self) -> Result<(), TransportError> {
        self.delivery
            .ack(BasicAckOptions::default())
            .await
            .map_err(|err| retryable("amqp ack", err))
    }

    async fn nack(self, _reason: &str) -> Result<(), TransportError> {
        // Requeue for redelivery.
        self.delivery
            .nack(BasicNackOptions {
                requeue: true,
                ..Default::default()
            })
            .await
            .map_err(|err| retryable("amqp nack", err))
    }

    async fn dead_letter(self, _reason: &str) -> Result<(), TransportError> {
        // Reject without requeue: routes to the queue's dead-letter exchange if
        // one is configured, otherwise drops.
        self.delivery
            .reject(BasicRejectOptions { requeue: false })
            .await
            .map_err(|err| retryable("amqp reject", err))
    }

    async fn park(self, _reason: &str) -> Result<(), TransportError> {
        self.delivery
            .reject(BasicRejectOptions { requeue: false })
            .await
            .map_err(|err| retryable("amqp reject", err))
    }
}

fn amqp_value_to_string(value: &AMQPValue) -> String {
    match value {
        AMQPValue::LongString(s) => s.to_string(),
        AMQPValue::ShortString(s) => s.to_string(),
        other => format!("{other:?}"),
    }
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
