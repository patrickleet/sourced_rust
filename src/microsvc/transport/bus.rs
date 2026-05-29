//! The ergonomic bus surface: `send`/`listen` (point-to-point commands) and
//! `publish`/`subscribe` (fan-out events), mirroring the Node `servicebus`
//! family (`rabbitbus`/`kafkabus`/`knativebus`).
//!
//! The surface is split so producing (uniform across every transport) and
//! consuming (a `run_source` loop for pull transports, but generated manifests
//! for Knative) stay coherent:
//!
//! - [`Bus`] — produce: `send` a command, `publish` an event. Every transport.
//! - [`BusConsumer`] — consume: `listen` for commands (competing), `subscribe`
//!   to events (fan-out). Pull transports only (in-memory, NATS, RabbitMQ,
//!   Kafka, Postgres). Knative consumes via generated Triggers + the HTTP
//!   ingress, so it implements only [`Bus`].
//!
//! A concrete `*Bus` implements both, so `bus.send/listen/publish/subscribe` all
//! work on it. `send`/`publish` lower to the transport's [`AsyncMessagePublisher`];
//! `listen`/`subscribe` build the transport's [`AsyncMessageSource`] with the
//! right topology and run it through the shared [`run_source`](super::run_source).

use std::future::Future;
use std::sync::Arc;

use super::{Message, RunOptions, TransportError};
use crate::microsvc::Service;

/// Produce side of the bus — uniform across every transport.
pub trait Bus: Send + Sync {
    /// Send a point-to-point command (1:1, competing consumers).
    fn send(
        &self,
        name: &str,
        payload: Vec<u8>,
    ) -> impl Future<Output = Result<(), TransportError>> + Send;

    /// Publish a fan-out event (1:N).
    fn publish(
        &self,
        name: &str,
        payload: Vec<u8>,
    ) -> impl Future<Output = Result<(), TransportError>> + Send;

    /// Send a fully-formed command message (explicit id/metadata/content-type).
    fn send_message(
        &self,
        message: Message,
    ) -> impl Future<Output = Result<(), TransportError>> + Send;

    /// Publish a fully-formed event message.
    fn publish_message(
        &self,
        message: Message,
    ) -> impl Future<Output = Result<(), TransportError>> + Send;
}

/// Consume side of the bus — pull transports that run a [`run_source`] loop.
///
/// `listen`/`subscribe` derive the message names from the service's registered
/// handlers ([`Service::command_names`]/[`Service::event_names`]), build the
/// transport's source with the matching topology, and run it. Both run until the
/// source drains/stops.
///
/// [`run_source`]: super::run_source
pub trait BusConsumer: Send + Sync {
    /// Run `service` as a command listener: consume its command names with
    /// competing-consumer (point-to-point) semantics.
    fn listen<D: Send + Sync + 'static>(
        &self,
        service: Arc<Service<D>>,
        options: RunOptions,
    ) -> impl Future<Output = Result<(), TransportError>> + Send;

    /// Run `service` as an event subscriber: consume its event names with
    /// fan-out semantics.
    fn subscribe<D: Send + Sync + 'static>(
        &self,
        service: Arc<Service<D>>,
        options: RunOptions,
    ) -> impl Future<Output = Result<(), TransportError>> + Send;
}
