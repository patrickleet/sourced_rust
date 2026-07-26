use std::future::Future;

#[cfg(feature = "metrics")]
use std::time::{Duration, Instant};

use super::{Message, TransportError};

#[cfg(feature = "metrics")]
use super::MessageKind;

#[derive(Clone, Copy, Debug)]
pub(crate) enum BusOperation {
    Send,
    Publish,
}

impl BusOperation {
    #[cfg(feature = "otel")]
    fn as_str(self) -> &'static str {
        match self {
            Self::Send => "send",
            Self::Publish => "publish",
        }
    }
}

pub(crate) async fn record_direct_publish<F, Fut>(
    service: Option<&str>,
    transport: &'static str,
    operation: BusOperation,
    message: Message,
    publish: F,
) -> Result<(), TransportError>
where
    F: FnOnce(Message) -> Fut,
    Fut: Future<Output = Result<(), TransportError>> + Send,
{
    #[cfg(not(feature = "metrics"))]
    let _ = service;
    #[cfg(not(feature = "otel"))]
    let _ = operation;
    #[cfg(not(any(feature = "metrics", feature = "otel")))]
    let _ = transport;

    #[cfg(feature = "metrics")]
    let started = Instant::now();
    #[cfg(feature = "metrics")]
    let kind = message.kind;

    #[cfg(feature = "otel")]
    let result = {
        use tracing::Instrument as _;

        let span =
            crate::telemetry::transport_publish_span(transport, operation.as_str(), &message);
        crate::trace_context::set_span_parent_from_metadata_if_no_current_span(
            &span,
            &message.metadata,
        );
        publish(message).instrument(span).await
    };

    #[cfg(not(feature = "otel"))]
    let result = publish(message).await;

    #[cfg(feature = "metrics")]
    record_direct_publish_result(service, transport, kind, started.elapsed(), &result);

    result
}

#[cfg(feature = "metrics")]
fn record_direct_publish_result(
    service: Option<&str>,
    transport: &str,
    kind: MessageKind,
    duration: Duration,
    result: &Result<(), TransportError>,
) {
    let outcome = match result {
        Ok(()) => crate::telemetry::transport_publish_outcome::PUBLISHED,
        Err(_) => crate::telemetry::transport_publish_outcome::FAILED,
    };
    crate::metrics::record_transport_publish(service, transport, kind, outcome, duration);

    if let Err(error) = result {
        crate::metrics::record_transport_publish_failure(
            service,
            transport,
            kind,
            crate::telemetry::transport_failure_class(error.kind()),
        );
    }
}

#[cfg(all(test, feature = "metrics"))]
mod tests {
    use super::*;
    use crate::bus::MessageKind;

    #[tokio::test]
    async fn failed_direct_publish_records_outcome_and_failure_class_without_ids() {
        let _guard = crate::metrics::async_lock_for_tests().await;
        crate::metrics::reset_for_tests();

        let message = Message::new("secret.direct.event", MessageKind::Event, b"{}".to_vec())
            .with_id("msg-secret-1")
            .with_metadata(
                "traceparent",
                "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
            );

        let result = record_direct_publish(
            Some("orders"),
            "in_memory",
            BusOperation::Publish,
            message,
            |_message| async { Err(TransportError::permanent("broker rejected")) },
        )
        .await;
        assert!(result.is_err());

        let text = crate::metrics::prometheus_text();
        assert!(text.contains(
            "distributed_transport_publish_total{service=\"orders\",transport=\"in_memory\",message_kind=\"event\",outcome=\"failed\"} 1"
        ));
        assert!(text.contains(
            "distributed_transport_publish_duration_seconds_count{service=\"orders\",transport=\"in_memory\",message_kind=\"event\",outcome=\"failed\"} 1"
        ));
        assert!(text.contains(
            "distributed_transport_publish_failures_total{service=\"orders\",transport=\"in_memory\",message_kind=\"event\",failure_class=\"permanent\"} 1"
        ));
        assert!(!text.contains("secret.direct.event"));
        assert!(!text.contains("msg-secret-1"));
        assert!(!text.contains("4bf92f3577b34da6a3ce929d0e0e4736"));
    }
}
