//! Shared vocabulary for async message transports.
//!
//! `microsvc` owns handler registration, guards, typed input decoding, and
//! dispatch. *Transport adapters* own how messages are received, acknowledged,
//! retried, published, and mapped to external topics/subjects/routes/triggers.
//! This module is the neutral vocabulary both sides share — it does not
//! implement any concrete broker.
//!
//! The direct-transport receive path lives here too:
//!
//! - [`AsyncMessageSource`] / [`ReceivedMessage`] — pull a message from a direct
//!   transport (Postgres, RabbitMQ, Kafka, NATS, in-memory) and settle it;
//! - [`MessageRouter`] — the consume seam a runner dispatches to, implemented by
//!   `microsvc::Service` and the dependency-free [`Handlers`] builder;
//! - [`run_source`] — the runner that dispatches each message through the
//!   consumer's [`MessageRouter::dispatch`], then acks/nacks/dead-letters per policy.
//!
//! The producer-side publish path lives here too:
//!
//! - [`AsyncMessagePublisher`] — the single publish boundary, with each adapter's
//!   durable publish threshold documented;
//! - [`OutboxDispatcher`](crate::OutboxDispatcher) / [`OutboxDispatchOutcome`](crate::OutboxDispatchOutcome) — map durable outbox rows to
//!   `Message` and dispatch them, sharing one claim → publish → complete path
//!   between background polling and after-commit immediate dispatch.
//!
//! Concrete adapters build on these traits: the Postgres outbox-backed source
//! ([`OutboxSource`](crate::OutboxSource)) is always available; the NATS JetStream and RabbitMQ
//! adapters are behind the `nats` and `rabbitmq` features. The Knative/HTTP
//! ingress shape and the Kafka adapter are still separate slices. Everything
//! builds on the vocabulary defined here:
//!
//! - [`TransportError`] / [`TransportErrorKind`] — retryable vs permanent
//!   classification the runner uses to decide between redelivery and the
//!   failure policy.
//! - [`FailurePolicy`] / [`FailureAction`] — what happens to a permanent
//!   failure: dead-letter, park, log-and-ack, retry, or stop.
//! - [`RunOptions`] / [`ConsumerDeliveryMode`] / [`InboxHook`] — idempotent
//!   dispatch by default, with a placeholder hook for the future consumer
//!   inbox.
//! - [`TransportCapabilities`] — how each transport differs in receive
//!   durability, publish confirmation, retry ownership, acknowledgement, and
//!   Knative integration.
//! - [`validate_stable_message_id`] — the rules an inbox-enabled run uses to
//!   reject messages that lack a usable deduplication key.
//!
//! # Two confirmation thresholds
//!
//! Producing and consuming have *separate* completion thresholds, and they must
//! not be conflated:
//!
//! **Producer publish threshold** — when an outbox row may be marked published.
//! Only after the adapter's durable publish confirmation:
//!
//! - Postgres: the outbox-backed bus row committed, or a committed insert into a
//!   separate queue table;
//! - RabbitMQ: publisher confirm;
//! - Kafka: the producer send acknowledged per the configured `acks`;
//! - NATS JetStream: a JetStream publish ack;
//! - Knative / HTTP: a successful response from the Broker/sink;
//! - in-memory: accepted into the in-memory queue/log.
//!
//! If the publish outcome is unknown, the outbox row stays retryable. Duplicate
//! delivery is acceptable under at-least-once semantics.
//!
//! **Consumer ack threshold** — when the adapter may acknowledge receipt. Only
//! after the runner reports successful consumer execution:
//!
//! - the guard passed (or the message was intentionally ignored by routing);
//! - the handler returned success;
//! - the handler's aggregate / read-model / outbox writes committed;
//! - in inbox mode, the inbox receipt committed atomically with those effects.
//!
//! How that acknowledgement maps back to the transport is adapter-owned and
//! described by [`ConsumerAckKind`]: a row completion, a delivery ack, an offset
//! commit, a stream ack, or a 2xx HTTP response. The default never silently
//! acknowledges a handler error — retryable failures redeliver and permanent
//! failures go through the [`FailurePolicy`].
//!
//! Producer-side immediate dispatch is *not* a transport acknowledgement: it is
//! best-effort delivery after the local transaction commits. Consumer-side
//! deduplication, when needed, is the optional consumer inbox, not an outbox or
//! publish guarantee.

mod bus;
mod capabilities;
mod error;
mod failure_policy;
mod handlers;
mod in_memory_bus;
#[cfg(feature = "kafka")]
mod kafka;
#[cfg(feature = "kafka")]
mod kafka_bus;
#[cfg(feature = "http")]
mod knative;
#[cfg(feature = "http")]
mod knative_bus;
mod message;
#[cfg(feature = "nats")]
mod nats;
#[cfg(feature = "nats")]
mod nats_bus;
#[cfg(feature = "postgres")]
mod postgres_bus;
mod publisher;
#[cfg(feature = "rabbitmq")]
mod rabbit_bus;
#[cfg(feature = "rabbitmq")]
mod rabbitmq;
mod router;
mod run_options;
mod runner;
mod source;
mod stable_id;

#[cfg(feature = "kafka")]
pub use kafka::{KafkaPublisher, KafkaReceived, KafkaSource};
#[cfg(feature = "kafka")]
pub use kafka_bus::KafkaBus;
#[cfg(feature = "http")]
pub use knative::knative_triggers;
#[cfg(feature = "http")]
pub use knative_bus::KnativeBus;
#[cfg(feature = "nats")]
pub use nats::{NatsJetStreamSource, NatsPublisher, NatsReceived};
#[cfg(feature = "nats")]
pub use nats_bus::NatsBus;
#[cfg(feature = "rabbitmq")]
pub use rabbit_bus::RabbitBus;
#[cfg(feature = "rabbitmq")]
pub use rabbitmq::{RabbitPublisher, RabbitReceived, RabbitSource};

pub use bus::{Bus, BusConsumer};
pub use capabilities::{ConsumerAckKind, KnativeIntegrationKind, TransportCapabilities};
pub use error::{TransportError, TransportErrorKind};
pub use failure_policy::{FailureAction, FailurePolicy};
pub use handlers::{AsyncMessageHandler, Handlers};
pub use in_memory_bus::{InMemoryBus, InMemoryReceived};
pub use message::{Message, MessageKind, PayloadDecodeError, SubscriptionPlan};
#[cfg(feature = "postgres")]
pub use postgres_bus::{LogReceived, PostgresBus, QueueReceived};
pub use publisher::{AsyncMessagePublisher, DynPublisher};
pub use router::MessageRouter;
pub use run_options::{ConsumerDeliveryMode, InboxHook, NoInbox, RunOptions};
pub use runner::run_source;
pub use source::{AsyncMessageSource, ReceivedMessage};
pub use stable_id::{validate_stable_message_id, StableMessageIdError, MAX_STABLE_MESSAGE_ID_LEN};
