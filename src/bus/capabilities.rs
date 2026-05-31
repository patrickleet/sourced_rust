//! Transport capability metadata.
//!
//! Different transports place receive durability, publish confirmation, retry
//! ownership, acknowledgement, and Knative integration in different places.
//! [`TransportCapabilities`] makes those differences explicit so docs, tests,
//! and adapter selection can reason about them instead of assuming every
//! transport behaves like one reference broker.
//!
//! The named constructors encode the comparison matrix from
//! `specs/transport-interface-evaluation`.

/// How a consumer acknowledges successful handler execution back to its
/// transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ConsumerAckKind {
    /// Complete / delete / archive a durable table row (Postgres).
    TableRow,
    /// Ack / nack a broker delivery (RabbitMQ / servicebus AMQP).
    DeliveryAck,
    /// Commit the consumer group offset (Kafka).
    OffsetCommit,
    /// Ack / nak / term a stream message (NATS JetStream).
    StreamAck,
    /// Return a successful HTTP response (Knative / CloudEvents).
    HttpResponse,
    /// In-process acknowledgement only (in-memory dev/test transport).
    InProcess,
}

/// How a transport integrates with Knative Eventing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum KnativeIntegrationKind {
    /// First-class Knative Eventing path (Kafka and RabbitMQ Broker/Source,
    /// Knative HTTP CloudEvents).
    Native,
    /// Needs a custom bridge that POSTs CloudEvents to a Knative Broker or is
    /// triggered by Knative and republishes to the transport (Postgres, NATS
    /// JetStream).
    CustomBridge,
    /// No Knative integration (in-memory dev/test transport).
    None,
}

/// Capability profile for a transport adapter.
///
/// The two confirmation thresholds are distinct and both matter:
///
/// - `publish_confirm` is the *producer* threshold an outbox row waits for
///   before it is marked published.
/// - `consumer_ack` is the *consumer* threshold the runner waits for before the
///   adapter acknowledges receipt.
///
/// `platform_managed_retry` distinguishes platform-driven delivery (Knative
/// owns retry, backoff, and dead-lettering) from direct transports where the
/// adapter and this crate own that operational contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct TransportCapabilities {
    /// The transport durably stores received messages until acknowledged (a
    /// table, queue, stream, or broker), rather than relying on a live process
    /// to hold them.
    pub durable_receive: bool,
    /// Publishing has a durable confirmation threshold (transaction commit,
    /// publisher confirm, producer ack, or a successful HTTP response).
    pub publish_confirm: bool,
    /// Retry, backoff, and dead-lettering are managed by the platform rather
    /// than by this crate and the adapter.
    pub platform_managed_retry: bool,
    /// How the consumer acknowledges successful execution.
    pub consumer_ack: ConsumerAckKind,
    /// How the transport integrates with Knative Eventing.
    pub knative_integration: KnativeIntegrationKind,
}

impl TransportCapabilities {
    /// Postgres: durable table-backed transport, transaction-commit publish
    /// confirmation, app-owned retry, row-completion ack, custom Knative bridge.
    pub const fn postgres() -> Self {
        Self {
            durable_receive: true,
            publish_confirm: true,
            platform_managed_retry: false,
            consumer_ack: ConsumerAckKind::TableRow,
            knative_integration: KnativeIntegrationKind::CustomBridge,
        }
    }

    /// RabbitMQ / servicebus: durable broker, publisher-confirm publish, app/
    /// broker-owned retry topology, delivery-ack consumer, native Knative path.
    pub const fn rabbitmq() -> Self {
        Self {
            durable_receive: true,
            publish_confirm: true,
            platform_managed_retry: false,
            consumer_ack: ConsumerAckKind::DeliveryAck,
            knative_integration: KnativeIntegrationKind::Native,
        }
    }

    /// Kafka: durable log, producer-ack publish, app-owned retry/DLQ topics,
    /// offset-commit ack, native Knative path.
    pub const fn kafka() -> Self {
        Self {
            durable_receive: true,
            publish_confirm: true,
            platform_managed_retry: false,
            consumer_ack: ConsumerAckKind::OffsetCommit,
            knative_integration: KnativeIntegrationKind::Native,
        }
    }

    /// NATS JetStream: durable stream, publish-ack publish, JetStream/custom
    /// retry, stream-ack consumer, custom Knative bridge.
    pub const fn nats_jetstream() -> Self {
        Self {
            durable_receive: true,
            publish_confirm: true,
            platform_managed_retry: false,
            consumer_ack: ConsumerAckKind::StreamAck,
            knative_integration: KnativeIntegrationKind::CustomBridge,
        }
    }

    /// Knative / HTTP CloudEvents: platform-delivered (no app-owned durable
    /// receive store), HTTP-response publish confirmation, platform-managed
    /// retry/backoff/dead-letter, HTTP-response ack, native integration.
    pub const fn knative() -> Self {
        Self {
            durable_receive: false,
            publish_confirm: true,
            platform_managed_retry: true,
            consumer_ack: ConsumerAckKind::HttpResponse,
            knative_integration: KnativeIntegrationKind::Native,
        }
    }

    /// In-memory dev/test transport: not durable, best-effort acceptance with no
    /// durable publish confirmation, no platform retry, in-process ack, no
    /// Knative integration.
    pub const fn in_memory() -> Self {
        Self {
            durable_receive: false,
            publish_confirm: false,
            platform_managed_retry: false,
            consumer_ack: ConsumerAckKind::InProcess,
            knative_integration: KnativeIntegrationKind::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn postgres_is_durable_with_custom_knative_bridge() {
        let caps = TransportCapabilities::postgres();
        assert!(caps.durable_receive);
        assert!(caps.publish_confirm);
        assert!(!caps.platform_managed_retry);
        assert_eq!(caps.consumer_ack, ConsumerAckKind::TableRow);
        assert_eq!(
            caps.knative_integration,
            KnativeIntegrationKind::CustomBridge
        );
    }

    #[test]
    fn knative_delegates_retry_to_the_platform() {
        let caps = TransportCapabilities::knative();
        assert!(caps.platform_managed_retry);
        assert!(!caps.durable_receive);
        assert_eq!(caps.consumer_ack, ConsumerAckKind::HttpResponse);
        assert_eq!(caps.knative_integration, KnativeIntegrationKind::Native);
    }

    #[test]
    fn in_memory_is_the_only_profile_without_durable_receive_or_publish_confirm() {
        let caps = TransportCapabilities::in_memory();
        assert!(!caps.durable_receive);
        assert!(!caps.publish_confirm);
        assert_eq!(caps.consumer_ack, ConsumerAckKind::InProcess);
        assert_eq!(caps.knative_integration, KnativeIntegrationKind::None);
    }

    #[test]
    fn direct_transports_own_their_retry() {
        for caps in [
            TransportCapabilities::postgres(),
            TransportCapabilities::rabbitmq(),
            TransportCapabilities::kafka(),
            TransportCapabilities::nats_jetstream(),
        ] {
            assert!(
                !caps.platform_managed_retry,
                "direct transports own retry; only Knative is platform-managed"
            );
            assert!(caps.durable_receive, "direct transports receive durably");
        }
    }

    #[test]
    fn each_known_transport_has_a_distinct_ack_kind() {
        let acks = [
            TransportCapabilities::postgres().consumer_ack,
            TransportCapabilities::rabbitmq().consumer_ack,
            TransportCapabilities::kafka().consumer_ack,
            TransportCapabilities::nats_jetstream().consumer_ack,
            TransportCapabilities::knative().consumer_ack,
            TransportCapabilities::in_memory().consumer_ack,
        ];
        for (i, a) in acks.iter().enumerate() {
            for b in &acks[i + 1..] {
                assert_ne!(a, b, "ack kinds should be distinct across transports");
            }
        }
    }

    #[test]
    fn knative_integration_matches_the_evaluation_matrix() {
        use KnativeIntegrationKind::*;
        // Native Eventing paths.
        assert_eq!(
            TransportCapabilities::rabbitmq().knative_integration,
            Native
        );
        assert_eq!(TransportCapabilities::kafka().knative_integration, Native);
        assert_eq!(TransportCapabilities::knative().knative_integration, Native);
        // Custom-bridge transports.
        assert_eq!(
            TransportCapabilities::postgres().knative_integration,
            CustomBridge
        );
        assert_eq!(
            TransportCapabilities::nats_jetstream().knative_integration,
            CustomBridge
        );
        // No integration.
        assert_eq!(TransportCapabilities::in_memory().knative_integration, None);
    }

    #[test]
    fn capabilities_round_trip_through_serde() {
        let caps = TransportCapabilities::kafka();
        let json = serde_json::to_string(&caps).unwrap();
        let restored: TransportCapabilities = serde_json::from_str(&json).unwrap();
        assert_eq!(caps, restored);
    }
}
