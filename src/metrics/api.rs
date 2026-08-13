use std::time::Duration;

use crate::bus::MessageKind;
use crate::telemetry::service_label;

use super::prometheus::render_prometheus;
use super::registry::{
    registry, DispatchKey, GraphqlRequestKey, MetricsSnapshot, OutboxMessageKey,
    TransportFailureKey, TransportMessageKey,
};

/// Record that a service exists so `/metrics` exposes a stable info series even
/// before the first request.
pub fn describe_service(service: Option<&str>) {
    registry().describe_service(service_label(service));
}

/// Record one microsvc command/event dispatch result.
pub fn record_microsvc_dispatch(
    service: Option<&str>,
    kind: MessageKind,
    message: &str,
    status: &str,
    duration: Duration,
) {
    registry().record_microsvc_dispatch(DispatchKey {
        service: service_label(service),
        message_kind: kind.as_str().to_string(),
        message: message.to_string(),
        status: status.to_string(),
        duration_seconds: duration.as_secs_f64(),
    });
}

/// Record a transport receive/settle outcome.
pub fn record_transport_message(
    service: Option<&str>,
    transport: &str,
    kind: MessageKind,
    outcome: &str,
) {
    registry().record_transport_message(TransportMessageKey {
        service: service_label(service),
        transport: transport.to_string(),
        message_kind: kind.as_str().to_string(),
        outcome: outcome.to_string(),
    });
}

/// Record a classified transport failure and the action chosen for it.
pub fn record_transport_failure(
    service: Option<&str>,
    transport: &str,
    failure_class: &str,
    action: &str,
) {
    registry().record_transport_failure(TransportFailureKey {
        service: service_label(service),
        transport: transport.to_string(),
        failure_class: failure_class.to_string(),
        action: action.to_string(),
    });
}

/// Record one GraphQL request (root field execution).
pub fn record_graphql_request(
    service: Option<&str>,
    root_field: &str,
    status: &str,
    duration: Duration,
) {
    registry().record_graphql_request(GraphqlRequestKey {
        service: service_label(service),
        root_field: root_field.to_string(),
        status: status.to_string(),
        duration_seconds: duration.as_secs_f64(),
    });
}

/// Record one outbox dispatch state transition.
pub fn record_outbox_message(service: Option<&str>, outcome: &str) {
    record_outbox_messages(service, outcome, 1);
}

/// Record `count` outbox dispatch state transitions that settled together.
pub fn record_outbox_messages(service: Option<&str>, outcome: &str, count: usize) {
    if count == 0 {
        return;
    }
    registry().record_outbox_messages(
        OutboxMessageKey {
            service: service_label(service),
            outcome: outcome.to_string(),
        },
        count as u64,
    );
}

/// Set outbox backlog gauges for a service.
pub fn set_outbox_backlog(
    service: Option<&str>,
    pending: usize,
    oldest_pending_age: Option<Duration>,
) {
    registry().set_outbox_backlog(
        service_label(service),
        pending as f64,
        oldest_pending_age.map(|duration| duration.as_secs_f64()),
    );
}

/// Render all currently recorded metrics in Prometheus text exposition format.
pub fn prometheus_text() -> String {
    render_prometheus(&snapshot())
}

/// Return a lock-free snapshot of bounded framework metric families.
///
/// The snapshot contains only metric names, bounded label names/values, and
/// numeric samples. It deliberately excludes payloads, message metadata, trace
/// identifiers, aggregate identifiers, and request-specific data so future
/// diagnostics can reuse it without widening the telemetry privacy surface.
pub(crate) fn snapshot() -> MetricsSnapshot {
    registry().snapshot()
}
