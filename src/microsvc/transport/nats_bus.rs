//! NATS JetStream [`Bus`] + [`BusConsumer`].
//!
//! One JetStream stream backs the bus, bound to `{namespace}.>`. The two bus
//! surfaces map onto subjects and durable consumers like this:
//!
//! - `send(name)` → subject `{namespace}.cmd.{name}` (command).
//! - `publish(name)` → subject `{namespace}.evt.{name}` (event).
//! - `listen` → a durable pull consumer named `{group}.cmd`, filtered to the
//!   service's command subjects. All replicas sharing a `group` bind the **same**
//!   durable, so JetStream load-balances commands across them — point-to-point /
//!   competing-consumer.
//! - `subscribe` → a durable pull consumer named `{group}.evt`, filtered to the
//!   service's event subjects. Each distinct `group` gets its **own** durable on
//!   the shared stream, so every group sees every event — fan-out.
//!
//! The `group` is the logical consumer identity (the service/deployment name).
//! Same group ⇒ competing; different groups ⇒ independent fan-out copies.
//!
//! Requires the `nats` feature. Integration-tested in `tests/nats_transport`.

use std::sync::Arc;
use std::time::Duration;

use async_nats::jetstream;
use async_nats::jetstream::consumer::pull::Config as PullConfig;
use async_nats::jetstream::stream::{Config as StreamConfig, Stream};

use super::nats::{NatsJetStreamSource, NatsPublisher};
use super::{run_source, AsyncMessagePublisher, Bus, BusConsumer, RunOptions, TransportError};
use crate::microsvc::{Message, MessageKind, Service};

const DEFAULT_FETCH_TIMEOUT: Duration = Duration::from_millis(500);

fn retryable(context: &str, err: impl std::fmt::Display) -> TransportError {
    TransportError::retryable(format!("{context}: {err}"))
}

/// NATS JetStream [`Bus`] + [`BusConsumer`]. Cheap to clone.
#[derive(Clone)]
pub struct NatsBus {
    jetstream: jetstream::Context,
    cmd_publisher: Arc<NatsPublisher>,
    evt_publisher: Arc<NatsPublisher>,
    group: String,
    namespace: String,
    stream_name: String,
    fetch_timeout: Duration,
}

impl NatsBus {
    /// Build a bus over an existing JetStream context.
    ///
    /// `group` is the logical consumer identity (same group ⇒ competing
    /// consumers; different groups ⇒ fan-out). `namespace` scopes the stream and
    /// subjects so multiple buses can share a server without collision.
    pub fn new(
        jetstream: jetstream::Context,
        group: impl Into<String>,
        namespace: impl Into<String>,
    ) -> Self {
        let namespace = namespace.into();
        let cmd_publisher =
            NatsPublisher::new(jetstream.clone()).with_subject_prefix(format!("{namespace}.cmd"));
        let evt_publisher =
            NatsPublisher::new(jetstream.clone()).with_subject_prefix(format!("{namespace}.evt"));
        Self {
            jetstream,
            cmd_publisher: Arc::new(cmd_publisher),
            evt_publisher: Arc::new(evt_publisher),
            group: group.into(),
            stream_name: namespace.to_uppercase().replace(['.', '-'], "_"),
            namespace,
            fetch_timeout: DEFAULT_FETCH_TIMEOUT,
        }
    }

    /// Connect to a NATS server URL and build a bus.
    pub async fn connect(
        url: &str,
        group: impl Into<String>,
        namespace: impl Into<String>,
    ) -> Result<Self, TransportError> {
        let client = async_nats::connect(url)
            .await
            .map_err(|err| retryable("nats connect", err))?;
        Ok(Self::new(jetstream::new(client), group, namespace))
    }

    /// Override how long a `listen`/`subscribe` poll waits before idling.
    pub fn with_fetch_timeout(mut self, timeout: Duration) -> Self {
        self.fetch_timeout = timeout;
        self
    }

    /// Sanitize the group into a valid NATS consumer-name token. Consumer names
    /// cannot contain `.`, `*`, `>`, or whitespace, so map them to `_`.
    fn durable_base(&self) -> String {
        self.group
            .chars()
            .map(|c| match c {
                '.' | '*' | '>' | ' ' | '\t' | '\n' | '/' | '\\' => '_',
                other => other,
            })
            .collect()
    }

    /// Create-or-open the backing stream (`{namespace}.>`). Called by
    /// `listen`/`subscribe`; producers should ensure it exists (here, via IaC, or
    /// by a consumer) before publishing, since JetStream rejects a publish to an
    /// unbound subject.
    pub async fn ensure_stream(&self) -> Result<Stream, TransportError> {
        self.jetstream
            .get_or_create_stream(StreamConfig {
                name: self.stream_name.clone(),
                subjects: vec![format!("{}.>", self.namespace)],
                ..Default::default()
            })
            .await
            .map_err(|err| retryable("nats get_or_create_stream", err))
    }

    /// Build a durable pull source over the bus stream, filtered to `subjects`,
    /// stripping `strip_prefix` so the dispatched message name is the bare name.
    async fn source(
        &self,
        durable: &str,
        subjects: Vec<String>,
        strip_prefix: String,
    ) -> Result<NatsJetStreamSource, TransportError> {
        let stream = self.ensure_stream().await?;
        let consumer = stream
            .get_or_create_consumer(
                durable,
                PullConfig {
                    durable_name: Some(durable.to_string()),
                    filter_subjects: subjects,
                    ..Default::default()
                },
            )
            .await
            .map_err(|err| retryable("nats get_or_create_consumer", err))?;
        Ok(NatsJetStreamSource::new(consumer)
            .with_fetch_timeout(self.fetch_timeout)
            .with_strip_prefix(strip_prefix))
    }
}

impl Bus for NatsBus {
    async fn send(&self, name: &str, payload: Vec<u8>) -> Result<(), TransportError> {
        self.send_message(Message::new(name, MessageKind::Command, payload))
            .await
    }

    async fn publish(&self, name: &str, payload: Vec<u8>) -> Result<(), TransportError> {
        self.publish_message(Message::new(name, MessageKind::Event, payload))
            .await
    }

    async fn send_message(&self, message: Message) -> Result<(), TransportError> {
        self.cmd_publisher.publish(message).await
    }

    async fn publish_message(&self, message: Message) -> Result<(), TransportError> {
        self.evt_publisher.publish(message).await
    }
}

impl BusConsumer for NatsBus {
    async fn listen<D: Send + Sync + 'static>(
        &self,
        service: Arc<Service<D>>,
        options: RunOptions,
    ) -> Result<(), TransportError> {
        let subjects: Vec<String> = service
            .command_names()
            .iter()
            .map(|name| format!("{}.cmd.{name}", self.namespace))
            .collect();
        if subjects.is_empty() {
            return Ok(());
        }
        let source = self
            .source(
                &format!("{}_cmd", self.durable_base()),
                subjects,
                format!("{}.cmd.", self.namespace),
            )
            .await?;
        run_source(service, source, options).await
    }

    async fn subscribe<D: Send + Sync + 'static>(
        &self,
        service: Arc<Service<D>>,
        options: RunOptions,
    ) -> Result<(), TransportError> {
        let subjects: Vec<String> = service
            .event_names()
            .iter()
            .map(|name| format!("{}.evt.{name}", self.namespace))
            .collect();
        if subjects.is_empty() {
            return Ok(());
        }
        let source = self
            .source(
                &format!("{}_evt", self.durable_base()),
                subjects,
                format!("{}.evt.", self.namespace),
            )
            .await?;
        run_source(service, source, options).await
    }
}
