//! Kafka transport adapter.
//!
//! [`KafkaPublisher`] sends a canonical [`Message`] to a topic named by the
//! message name, awaiting the producer ack (the durable publish threshold, per
//! the configured `acks`). [`KafkaSource`] consumes with a consumer group
//! (auto-commit disabled) and settles by offset: ack→commit the offset,
//! nack→seek back so the record is re-read, dead-letter/park→commit (skip).
//!
//! Offset commits use `CommitMode::Async` so settling never blocks the tokio
//! worker; this is at-least-once: a crash before an async commit lands redelivers
//! the record, so consumers must tolerate duplicates.
//!
//! Requires the `kafka` feature (builds `librdkafka` via cmake). Integration-
//! tested in `tests/kafka_transport` against a broker (see `compose.yaml`).

use std::sync::Arc;
use std::time::{Duration, Instant};

use rdkafka::config::ClientConfig;
use rdkafka::consumer::{Consumer, StreamConsumer};
use rdkafka::message::{Header, Headers, OwnedHeaders};
use rdkafka::producer::{FutureProducer, FutureRecord};
use rdkafka::{Message as KafkaMessageTrait, Offset, TopicPartitionList};

use super::source::{MessageSource, ReceivedMessage};
use super::{retryable, MessagePublisher, TransportError};
use super::{strip_address_prefix, Message, MessageKind};

const MESSAGE_ID_HEADER: &str = "x-sourced-id";
const MESSAGE_KIND_HEADER: &str = "x-sourced-kind";

/// Publishes canonical messages to a Kafka topic named by the message name.
pub struct KafkaPublisher {
    producer: FutureProducer,
    send_timeout: Duration,
}

impl KafkaPublisher {
    /// Wrap an existing producer.
    pub fn new(producer: FutureProducer) -> Self {
        Self {
            producer,
            send_timeout: Duration::from_secs(10),
        }
    }

    /// Connect a producer to `brokers` (comma-separated `host:port`), waiting for
    /// `acks=all` so a successful send is durably replicated.
    pub async fn connect(brokers: &str) -> Result<Self, TransportError> {
        let producer: FutureProducer = ClientConfig::new()
            .set("bootstrap.servers", brokers)
            .set("acks", "all")
            .set("message.timeout.ms", "10000")
            .create()
            .map_err(|err| retryable("kafka producer", err))?;
        Ok(Self::new(producer))
    }
}

fn owned_headers(message: &Message) -> OwnedHeaders {
    let mut headers = OwnedHeaders::new().insert(Header {
        key: MESSAGE_KIND_HEADER,
        value: Some(message.kind.as_str()),
    });
    if let Some(id) = message.id() {
        headers = headers.insert(Header {
            key: MESSAGE_ID_HEADER,
            value: Some(id),
        });
    }
    for (key, value) in &message.metadata {
        headers = headers.insert(Header {
            key: key.as_str(),
            value: Some(value.as_str()),
        });
    }
    headers
}

impl MessagePublisher for KafkaPublisher {
    async fn publish(&self, message: Message) -> Result<(), TransportError> {
        let topic = message.name().to_string();
        let key = message.id().unwrap_or(message.name()).to_string();
        let headers = owned_headers(&message);
        let record = FutureRecord::to(&topic)
            .payload(&message.payload)
            .key(&key)
            .headers(headers);
        self.producer
            .send(record, self.send_timeout)
            .await
            .map_err(|(err, _)| retryable("kafka send", err))?;
        Ok(())
    }
}

/// Consumes a topic with a consumer group, committing offsets on ack.
pub struct KafkaSource {
    consumer: Arc<StreamConsumer>,
    fetch_timeout: Duration,
    strip_prefix: Option<String>,
}

impl KafkaSource {
    /// Wrap an existing subscribed consumer.
    pub fn new(consumer: Arc<StreamConsumer>) -> Self {
        Self {
            consumer,
            fetch_timeout: Duration::from_secs(5),
            strip_prefix: None,
        }
    }

    /// How long `recv` waits for a record before returning `Ok(None)`.
    pub fn with_fetch_timeout(mut self, timeout: Duration) -> Self {
        self.fetch_timeout = timeout;
        self
    }

    /// Strip `prefix` from each record's topic when deriving the message name, so
    /// a topic `app.cmd.account.debit` becomes the name `account.debit`. Used by
    /// [`KafkaBus`](super::KafkaBus); default: no stripping (the topic is the name).
    pub fn with_strip_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.strip_prefix = Some(prefix.into());
        self
    }

    /// Connect a consumer (group `group_id`, auto-commit off, earliest reset) and
    /// subscribe to `topics`.
    pub async fn connect(
        brokers: &str,
        group_id: &str,
        topics: &[&str],
    ) -> Result<Self, TransportError> {
        let consumer: StreamConsumer = ClientConfig::new()
            .set("bootstrap.servers", brokers)
            .set("group.id", group_id)
            .set("enable.auto.commit", "false")
            .set("auto.offset.reset", "earliest")
            .create()
            .map_err(|err| retryable("kafka consumer", err))?;
        consumer
            .subscribe(topics)
            .map_err(|err| retryable("kafka subscribe", err))?;
        Ok(Self::new(Arc::new(consumer)))
    }
}

impl MessageSource for KafkaSource {
    type Received = KafkaReceived;

    async fn recv(&mut self) -> Result<Option<Self::Received>, TransportError> {
        // Poll within the fetch-timeout budget. Kafka surfaces transient broker
        // transport/coordination errors (normal during group bootstrap and
        // rebalances) as recv errors; the client recovers, so we retry until a
        // message arrives or the budget elapses (drain → None), rather than
        // ending the run on a transient hiccup.
        let deadline = Instant::now() + self.fetch_timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(None);
            }
            match tokio::time::timeout(remaining, self.consumer.recv()).await {
                Ok(Ok(borrowed)) => {
                    return Ok(Some(KafkaReceived::from_borrowed(
                        &borrowed,
                        self.consumer.clone(),
                        self.strip_prefix.as_deref(),
                    )));
                }
                Ok(Err(_transient)) => {
                    // Back off briefly, then retry within the remaining budget.
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                Err(_elapsed) => return Ok(None),
            }
        }
    }
}

/// A consumed record plus the means to commit/seek its offset.
pub struct KafkaReceived {
    consumer: Arc<StreamConsumer>,
    topic: String,
    partition: i32,
    offset: i64,
    message: Message,
}

impl KafkaReceived {
    fn from_borrowed(
        borrowed: &rdkafka::message::BorrowedMessage<'_>,
        consumer: Arc<StreamConsumer>,
        strip_prefix: Option<&str>,
    ) -> Self {
        let payload = borrowed.payload().map(|p| p.to_vec()).unwrap_or_default();
        let topic = borrowed.topic().to_string();
        let name = strip_address_prefix(topic.clone(), strip_prefix);
        let mut id = None;
        let mut kind = MessageKind::Event;
        let mut metadata = Vec::new();
        if let Some(headers) = borrowed.headers() {
            for header in headers.iter() {
                let value = header
                    .value
                    .map(|v| String::from_utf8_lossy(v).into_owned())
                    .unwrap_or_default();
                match header.key {
                    MESSAGE_ID_HEADER => id = Some(value),
                    MESSAGE_KIND_HEADER => kind = MessageKind::from_str_lossy(&value),
                    other => metadata.push((other.to_string(), value)),
                }
            }
        }
        let mut message = Message::new(name, kind, payload);
        message.id = id;
        message.metadata = metadata;
        Self {
            consumer,
            topic,
            partition: borrowed.partition(),
            offset: borrowed.offset(),
            message,
        }
    }

    /// Commit this record's offset (the next offset to read) without blocking the
    /// async runtime.
    ///
    /// Uses [`CommitMode::Async`]: the offset is handed to librdkafka's background
    /// thread and this returns immediately, rather than blocking the tokio worker
    /// on a broker round trip as `CommitMode::Sync` does. This keeps at-least-once
    /// semantics — the runner already acks only after handler effects complete, so
    /// a crash between effect and the async commit landing simply redelivers the
    /// record (a duplicate the consumer must tolerate, which it already does).
    fn commit_offset(&self) -> Result<(), TransportError> {
        let mut tpl = TopicPartitionList::new();
        tpl.add_partition_offset(&self.topic, self.partition, Offset::Offset(self.offset + 1))
            .map_err(|err| retryable("kafka offset", err))?;
        self.consumer
            .commit(&tpl, rdkafka::consumer::CommitMode::Async)
            .map_err(|err| retryable("kafka commit", err))
    }
}

impl ReceivedMessage for KafkaReceived {
    fn message(&self) -> &Message {
        &self.message
    }

    async fn ack(self) -> Result<(), TransportError> {
        self.commit_offset()
    }

    async fn nack(self, _reason: &str) -> Result<(), TransportError> {
        // Do not commit; seek back so this record is re-read (redelivery).
        //
        // `seek` blocks the calling thread until it takes effect (up to the
        // timeout) and rdkafka exposes no async variant, so run it on the
        // blocking pool to avoid stalling the tokio worker. The consumer is
        // Arc-shared and `seek` takes `&self`, so a clone moves cleanly into the
        // blocking task.
        let consumer = self.consumer.clone();
        let topic = self.topic;
        let partition = self.partition;
        let offset = self.offset;
        tokio::task::spawn_blocking(move || {
            consumer.seek(
                &topic,
                partition,
                Offset::Offset(offset),
                Duration::from_secs(5),
            )
        })
        .await
        .map_err(|err| retryable("kafka seek task", err))?
        .map_err(|err| retryable("kafka seek", err))
    }

    async fn dead_letter(self, _reason: &str) -> Result<(), TransportError> {
        // Skip the poison record by committing past it. A DLQ-topic producer is a
        // follow-up.
        self.commit_offset()
    }

    async fn park(self, _reason: &str) -> Result<(), TransportError> {
        self.commit_offset()
    }
}
