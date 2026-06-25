//! Prometheus-compatible framework metrics.
//!
//! This module intentionally has no SDK dependency. It records the framework
//! counters and gauges Distributed owns, then renders Prometheus text exposition
//! for HTTP scrape endpoints.

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use crate::bus::MessageKind;

const UNNAMED_SERVICE: &str = "unnamed";
#[cfg(feature = "http")]
const PROMETHEUS_TEXT_CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";
const HISTOGRAM_BUCKETS: [f64; 11] = [
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
];

static REGISTRY: OnceLock<MetricsRegistry> = OnceLock::new();

#[cfg(test)]
static TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

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
    registry().prometheus_text()
}

/// Build an axum router that exposes only `GET /metrics`.
///
/// This is intended for workers and services whose primary transport is not
/// HTTP. Run it on a small side port so Prometheus can scrape the same
/// framework metrics that the bus/outbox/runtime paths record.
#[cfg(feature = "http")]
pub fn http_router() -> axum::Router {
    http_router_with_state(MetricsHttpState::default())
}

/// Build an axum router that exposes only `GET /metrics` and records a stable
/// service label before each scrape.
#[cfg(feature = "http")]
pub fn http_router_for_service(service: impl Into<String>) -> axum::Router {
    http_router_with_state(MetricsHttpState {
        service: Some(service.into()),
    })
}

/// Serve only the metrics scrape endpoint at the given address.
///
/// This helper is deliberately independent of `microsvc::http`, so a NATS,
/// Kafka, RabbitMQ, or outbox worker can expose Prometheus metrics without
/// exposing command dispatch over HTTP.
#[cfg(feature = "http")]
pub async fn serve_http(addr: &str, service: Option<&str>) -> Result<(), std::io::Error> {
    let app = match service {
        Some(service) => http_router_for_service(service),
        None => http_router(),
    };
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await
}

/// Return a Prometheus text response for HTTP handlers.
#[cfg(feature = "http")]
pub fn prometheus_response(service: Option<&str>) -> impl axum::response::IntoResponse {
    describe_service(service);
    (
        [(
            axum::http::header::CONTENT_TYPE,
            PROMETHEUS_TEXT_CONTENT_TYPE,
        )],
        prometheus_text(),
    )
}

#[cfg(test)]
pub(crate) fn reset_for_tests() {
    registry().reset();
}

#[cfg(test)]
pub(crate) fn lock_for_tests() -> MutexGuard<'static, ()> {
    TEST_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("metrics test lock poisoned")
}

fn registry() -> &'static MetricsRegistry {
    REGISTRY.get_or_init(MetricsRegistry::default)
}

fn service_label(service: Option<&str>) -> String {
    service
        .filter(|value| !value.is_empty())
        .unwrap_or(UNNAMED_SERVICE)
        .to_string()
}

#[cfg(feature = "http")]
#[derive(Clone, Default)]
struct MetricsHttpState {
    service: Option<String>,
}

#[cfg(feature = "http")]
fn http_router_with_state(state: MetricsHttpState) -> axum::Router {
    axum::Router::new()
        .route("/metrics", axum::routing::get(metrics_http_handler))
        .with_state(state)
}

#[cfg(feature = "http")]
async fn metrics_http_handler(
    axum::extract::State(state): axum::extract::State<MetricsHttpState>,
) -> impl axum::response::IntoResponse {
    prometheus_response(state.service.as_deref())
}

#[derive(Default)]
struct MetricsRegistry {
    inner: Mutex<MetricsInner>,
}

impl MetricsRegistry {
    fn describe_service(&self, service: String) {
        self.inner().service_info.insert(service, ());
    }

    fn record_microsvc_dispatch(&self, key: DispatchKey) {
        let mut inner = self.inner();
        inner
            .dispatch_total
            .entry(key.counter_key())
            .and_modify(|value| *value += 1)
            .or_insert(1);
        inner
            .dispatch_duration
            .entry(key.histogram_key())
            .or_insert_with(Histogram::new)
            .observe(key.duration_seconds);
        inner.service_info.insert(key.service, ());
    }

    fn record_transport_message(&self, key: TransportMessageKey) {
        let mut inner = self.inner();
        inner
            .transport_messages_total
            .entry(key.clone())
            .and_modify(|value| *value += 1)
            .or_insert(1);
        inner.service_info.insert(key.service, ());
    }

    fn record_transport_failure(&self, key: TransportFailureKey) {
        let mut inner = self.inner();
        inner
            .transport_failures_total
            .entry(key.clone())
            .and_modify(|value| *value += 1)
            .or_insert(1);
        inner.service_info.insert(key.service, ());
    }

    fn record_outbox_messages(&self, key: OutboxMessageKey, count: u64) {
        let mut inner = self.inner();
        inner
            .outbox_messages_total
            .entry(key.clone())
            .and_modify(|value| *value += count)
            .or_insert(count);
        inner.service_info.insert(key.service, ());
    }

    fn set_outbox_backlog(&self, service: String, pending: f64, oldest_pending_age: Option<f64>) {
        let mut inner = self.inner();
        inner
            .outbox_pending_messages
            .insert(service.clone(), pending);
        if let Some(age) = oldest_pending_age {
            inner
                .outbox_oldest_pending_age_seconds
                .insert(service.clone(), age);
        } else {
            inner.outbox_oldest_pending_age_seconds.remove(&service);
        }
        inner.service_info.insert(service, ());
    }

    fn prometheus_text(&self) -> String {
        let inner = self.inner();
        let mut output = String::new();

        write_help_type(
            &mut output,
            "distributed_service_info",
            "Static Distributed service metadata.",
            "gauge",
        );
        for service in inner.service_info.keys() {
            push_metric(
                &mut output,
                "distributed_service_info",
                &[
                    ("service", service.as_str()),
                    ("version", env!("CARGO_PKG_VERSION")),
                ],
                "1",
            );
        }

        write_help_type(
            &mut output,
            "distributed_microsvc_dispatch_total",
            "Total microsvc dispatches by service, message kind, message, and status.",
            "counter",
        );
        for (key, value) in &inner.dispatch_total {
            push_metric(
                &mut output,
                "distributed_microsvc_dispatch_total",
                &key.labels(),
                &value.to_string(),
            );
        }

        write_help_type(
            &mut output,
            "distributed_microsvc_dispatch_duration_seconds",
            "Microsvc dispatch duration in seconds.",
            "histogram",
        );
        for (key, histogram) in &inner.dispatch_duration {
            for (bucket, count) in HISTOGRAM_BUCKETS.iter().zip(histogram.bucket_counts.iter()) {
                let le = bucket.to_string();
                let mut labels = key.labels();
                labels.push(("le", le.as_str()));
                push_metric(
                    &mut output,
                    "distributed_microsvc_dispatch_duration_seconds_bucket",
                    &labels,
                    &count.to_string(),
                );
            }
            let mut labels = key.labels();
            labels.push(("le", "+Inf"));
            push_metric(
                &mut output,
                "distributed_microsvc_dispatch_duration_seconds_bucket",
                &labels,
                &histogram.count.to_string(),
            );
            push_metric(
                &mut output,
                "distributed_microsvc_dispatch_duration_seconds_sum",
                &key.labels(),
                &format_float(histogram.sum),
            );
            push_metric(
                &mut output,
                "distributed_microsvc_dispatch_duration_seconds_count",
                &key.labels(),
                &histogram.count.to_string(),
            );
        }

        write_help_type(
            &mut output,
            "distributed_transport_messages_total",
            "Total transport receive/settle outcomes.",
            "counter",
        );
        for (key, value) in &inner.transport_messages_total {
            push_metric(
                &mut output,
                "distributed_transport_messages_total",
                &key.labels(),
                &value.to_string(),
            );
        }

        write_help_type(
            &mut output,
            "distributed_transport_failures_total",
            "Total transport failures by class and chosen action.",
            "counter",
        );
        for (key, value) in &inner.transport_failures_total {
            push_metric(
                &mut output,
                "distributed_transport_failures_total",
                &key.labels(),
                &value.to_string(),
            );
        }

        write_help_type(
            &mut output,
            "distributed_outbox_messages_total",
            "Total outbox publish outcomes.",
            "counter",
        );
        for (key, value) in &inner.outbox_messages_total {
            push_metric(
                &mut output,
                "distributed_outbox_messages_total",
                &key.labels(),
                &value.to_string(),
            );
        }

        write_help_type(
            &mut output,
            "distributed_outbox_pending_messages",
            "Pending outbox message count.",
            "gauge",
        );
        for (service, value) in &inner.outbox_pending_messages {
            push_metric(
                &mut output,
                "distributed_outbox_pending_messages",
                &[("service", service.as_str())],
                &format_float(*value),
            );
        }

        write_help_type(
            &mut output,
            "distributed_outbox_oldest_pending_age_seconds",
            "Age in seconds of the oldest pending outbox message.",
            "gauge",
        );
        for (service, value) in &inner.outbox_oldest_pending_age_seconds {
            push_metric(
                &mut output,
                "distributed_outbox_oldest_pending_age_seconds",
                &[("service", service.as_str())],
                &format_float(*value),
            );
        }

        output.push_str("# EOF\n");
        output
    }

    #[cfg(test)]
    fn reset(&self) {
        *self.inner() = MetricsInner::default();
    }

    fn inner(&self) -> MutexGuard<'_, MetricsInner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[derive(Default)]
struct MetricsInner {
    service_info: BTreeMap<String, ()>,
    dispatch_total: BTreeMap<DispatchCounterKey, u64>,
    dispatch_duration: BTreeMap<DispatchHistogramKey, Histogram>,
    transport_messages_total: BTreeMap<TransportMessageKey, u64>,
    transport_failures_total: BTreeMap<TransportFailureKey, u64>,
    outbox_messages_total: BTreeMap<OutboxMessageKey, u64>,
    outbox_pending_messages: BTreeMap<String, f64>,
    outbox_oldest_pending_age_seconds: BTreeMap<String, f64>,
}

#[derive(Clone)]
struct DispatchKey {
    service: String,
    message_kind: String,
    message: String,
    status: String,
    duration_seconds: f64,
}

impl DispatchKey {
    fn counter_key(&self) -> DispatchCounterKey {
        DispatchCounterKey {
            service: self.service.clone(),
            message_kind: self.message_kind.clone(),
            message: self.message.clone(),
            status: self.status.clone(),
        }
    }

    fn histogram_key(&self) -> DispatchHistogramKey {
        DispatchHistogramKey {
            service: self.service.clone(),
            message_kind: self.message_kind.clone(),
            message: self.message.clone(),
            status: self.status.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct DispatchCounterKey {
    service: String,
    message_kind: String,
    message: String,
    status: String,
}

impl DispatchCounterKey {
    fn labels(&self) -> Vec<(&str, &str)> {
        vec![
            ("service", self.service.as_str()),
            ("message_kind", self.message_kind.as_str()),
            ("message", self.message.as_str()),
            ("status", self.status.as_str()),
        ]
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct DispatchHistogramKey {
    service: String,
    message_kind: String,
    message: String,
    status: String,
}

impl DispatchHistogramKey {
    fn labels(&self) -> Vec<(&str, &str)> {
        vec![
            ("service", self.service.as_str()),
            ("message_kind", self.message_kind.as_str()),
            ("message", self.message.as_str()),
            ("status", self.status.as_str()),
        ]
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct TransportMessageKey {
    service: String,
    transport: String,
    message_kind: String,
    outcome: String,
}

impl TransportMessageKey {
    fn labels(&self) -> Vec<(&str, &str)> {
        vec![
            ("service", self.service.as_str()),
            ("transport", self.transport.as_str()),
            ("message_kind", self.message_kind.as_str()),
            ("outcome", self.outcome.as_str()),
        ]
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct TransportFailureKey {
    service: String,
    transport: String,
    failure_class: String,
    action: String,
}

impl TransportFailureKey {
    fn labels(&self) -> Vec<(&str, &str)> {
        vec![
            ("service", self.service.as_str()),
            ("transport", self.transport.as_str()),
            ("failure_class", self.failure_class.as_str()),
            ("action", self.action.as_str()),
        ]
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct OutboxMessageKey {
    service: String,
    outcome: String,
}

impl OutboxMessageKey {
    fn labels(&self) -> Vec<(&str, &str)> {
        vec![
            ("service", self.service.as_str()),
            ("outcome", self.outcome.as_str()),
        ]
    }
}

#[derive(Clone)]
struct Histogram {
    bucket_counts: [u64; HISTOGRAM_BUCKETS.len()],
    sum: f64,
    count: u64,
}

impl Histogram {
    fn new() -> Self {
        Self {
            bucket_counts: [0; HISTOGRAM_BUCKETS.len()],
            sum: 0.0,
            count: 0,
        }
    }

    fn observe(&mut self, value: f64) {
        self.sum += value;
        self.count += 1;
        for (bucket, count) in HISTOGRAM_BUCKETS.iter().zip(self.bucket_counts.iter_mut()) {
            if value <= *bucket {
                *count += 1;
            }
        }
    }
}

fn write_help_type(output: &mut String, name: &str, help: &str, metric_type: &str) {
    output.push_str("# HELP ");
    output.push_str(name);
    output.push(' ');
    output.push_str(help);
    output.push('\n');
    output.push_str("# TYPE ");
    output.push_str(name);
    output.push(' ');
    output.push_str(metric_type);
    output.push('\n');
}

fn push_metric(output: &mut String, name: &str, labels: &[(&str, &str)], value: &str) {
    output.push_str(name);
    if !labels.is_empty() {
        output.push('{');
        for (index, (key, value)) in labels.iter().enumerate() {
            if index > 0 {
                output.push(',');
            }
            output.push_str(key);
            output.push_str("=\"");
            push_escaped_label_value(output, value);
            output.push('"');
        }
        output.push('}');
    }
    output.push(' ');
    output.push_str(value);
    output.push('\n');
}

fn push_escaped_label_value(output: &mut String, value: &str) {
    for ch in value.chars() {
        match ch {
            '\\' => output.push_str("\\\\"),
            '"' => output.push_str("\\\""),
            '\n' => output.push_str("\\n"),
            _ => output.push(ch),
        }
    }
}

fn format_float(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prometheus_text_escapes_label_values() {
        let _guard = lock_for_tests();
        reset_for_tests();

        record_microsvc_dispatch(
            Some("svc\"one"),
            MessageKind::Command,
            "line\nbreak",
            "success",
            Duration::from_millis(2),
        );

        let text = prometheus_text();

        assert!(text.contains(
            "distributed_microsvc_dispatch_total{service=\"svc\\\"one\",message_kind=\"command\",message=\"line\\nbreak\",status=\"success\"} 1"
        ));
    }

    #[test]
    fn histogram_records_cumulative_buckets() {
        let _guard = lock_for_tests();
        reset_for_tests();

        record_microsvc_dispatch(
            Some("orders"),
            MessageKind::Event,
            "orders.created",
            "success",
            Duration::from_millis(7),
        );

        let text = prometheus_text();

        assert!(text.contains(
            "distributed_microsvc_dispatch_duration_seconds_bucket{service=\"orders\",message_kind=\"event\",message=\"orders.created\",status=\"success\",le=\"0.005\"} 0"
        ));
        assert!(text.contains(
            "distributed_microsvc_dispatch_duration_seconds_bucket{service=\"orders\",message_kind=\"event\",message=\"orders.created\",status=\"success\",le=\"0.01\"} 1"
        ));
        assert!(text.contains(
            "distributed_microsvc_dispatch_duration_seconds_count{service=\"orders\",message_kind=\"event\",message=\"orders.created\",status=\"success\"} 1"
        ));
    }
}
