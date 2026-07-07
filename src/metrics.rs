//! Prometheus-compatible framework metrics.
//!
//! This module intentionally has no SDK dependency. It records the framework
//! counters and gauges Distributed owns, then renders Prometheus text exposition
//! for HTTP scrape endpoints.

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard as StdMutexGuard, OnceLock};
use std::time::Duration;

use crate::bus::MessageKind;
use crate::telemetry::{metric_labels, metric_names, service_label};

#[cfg(feature = "http")]
const PROMETHEUS_TEXT_CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";
const HISTOGRAM_BUCKETS: [f64; 11] = [
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
];
const SERVICE_INFO_FAMILY: MetricFamily = MetricFamily::gauge(
    metric_names::SERVICE_INFO,
    "Static Distributed service metadata.",
);
const MICROSVC_DISPATCH_TOTAL_FAMILY: MetricFamily = MetricFamily::counter(
    metric_names::MICROSVC_DISPATCH_TOTAL,
    "Total microsvc dispatches by service, message kind, message, and status.",
);
const MICROSVC_DISPATCH_DURATION_FAMILY: MetricFamily = MetricFamily::histogram(
    metric_names::MICROSVC_DISPATCH_DURATION_SECONDS,
    "Microsvc dispatch duration in seconds.",
);
const TRANSPORT_MESSAGES_TOTAL_FAMILY: MetricFamily = MetricFamily::counter(
    metric_names::TRANSPORT_MESSAGES_TOTAL,
    "Total transport receive/settle outcomes.",
);
const TRANSPORT_FAILURES_TOTAL_FAMILY: MetricFamily = MetricFamily::counter(
    metric_names::TRANSPORT_FAILURES_TOTAL,
    "Total transport failures by class and chosen action.",
);
const TRANSPORT_PUBLISH_TOTAL_FAMILY: MetricFamily = MetricFamily::counter(
    metric_names::TRANSPORT_PUBLISH_TOTAL,
    "Total direct transport publish outcomes.",
);
const TRANSPORT_PUBLISH_DURATION_FAMILY: MetricFamily = MetricFamily::histogram(
    metric_names::TRANSPORT_PUBLISH_DURATION_SECONDS,
    "Direct transport publish duration in seconds.",
);
const TRANSPORT_PUBLISH_FAILURES_TOTAL_FAMILY: MetricFamily = MetricFamily::counter(
    metric_names::TRANSPORT_PUBLISH_FAILURES_TOTAL,
    "Total direct transport publish failures by class.",
);
const OUTBOX_MESSAGES_TOTAL_FAMILY: MetricFamily = MetricFamily::counter(
    metric_names::OUTBOX_MESSAGES_TOTAL,
    "Total outbox publish outcomes.",
);
const OUTBOX_PENDING_MESSAGES_FAMILY: MetricFamily = MetricFamily::gauge(
    metric_names::OUTBOX_PENDING_MESSAGES,
    "Pending outbox message count.",
);
const OUTBOX_OLDEST_PENDING_AGE_FAMILY: MetricFamily = MetricFamily::gauge(
    metric_names::OUTBOX_OLDEST_PENDING_AGE_SECONDS,
    "Age in seconds of the oldest pending outbox message.",
);

static REGISTRY: OnceLock<MetricsRegistry> = OnceLock::new();

#[cfg(test)]
static TEST_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

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

/// Record one direct producer publish attempt.
pub fn record_transport_publish(
    service: Option<&str>,
    transport: &str,
    kind: MessageKind,
    outcome: &str,
    duration: Duration,
) {
    registry().record_transport_publish(TransportPublishKey {
        service: service_label(service),
        transport: transport.to_string(),
        message_kind: kind.as_str().to_string(),
        outcome: outcome.to_string(),
        duration_seconds: duration.as_secs_f64(),
    });
}

/// Record a classified direct producer publish failure.
pub fn record_transport_publish_failure(
    service: Option<&str>,
    transport: &str,
    kind: MessageKind,
    failure_class: &str,
) {
    registry().record_transport_publish_failure(TransportPublishFailureKey {
        service: service_label(service),
        transport: transport.to_string(),
        message_kind: kind.as_str().to_string(),
        failure_class: failure_class.to_string(),
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

/// Build an axum router that exposes only `GET /metrics`.
///
/// This is intended for workers and services whose primary transport is not
/// HTTP. Run it on a small side port so Prometheus can scrape the same
/// framework metrics that the bus/outbox/runtime paths record. The endpoint is
/// unauthenticated; bind it only on a private listener or behind equivalent
/// network controls.
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
/// exposing command dispatch over HTTP. The endpoint is unauthenticated; do not
/// bind it on a public interface unless an ingress or network policy restricts
/// access.
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
pub(crate) fn lock_for_tests() -> tokio::sync::MutexGuard<'static, ()> {
    TEST_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .blocking_lock()
}

#[cfg(test)]
pub(crate) async fn async_lock_for_tests() -> tokio::sync::MutexGuard<'static, ()> {
    TEST_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await
}

fn registry() -> &'static MetricsRegistry {
    REGISTRY.get_or_init(MetricsRegistry::default)
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MetricFamily {
    pub(crate) name: &'static str,
    help: &'static str,
    kind: MetricKind,
}

impl MetricFamily {
    const fn counter(name: &'static str, help: &'static str) -> Self {
        Self {
            name,
            help,
            kind: MetricKind::Counter,
        }
    }

    const fn gauge(name: &'static str, help: &'static str) -> Self {
        Self {
            name,
            help,
            kind: MetricKind::Gauge,
        }
    }

    const fn histogram(name: &'static str, help: &'static str) -> Self {
        Self {
            name,
            help,
            kind: MetricKind::Histogram,
        }
    }

    fn snapshot(self, samples: Vec<MetricSample>) -> MetricFamilySnapshot {
        MetricFamilySnapshot {
            family: self,
            samples,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MetricKind {
    Counter,
    Gauge,
    Histogram,
}

impl MetricKind {
    fn as_prometheus_type(self) -> &'static str {
        match self {
            Self::Counter => "counter",
            Self::Gauge => "gauge",
            Self::Histogram => "histogram",
        }
    }
}

/// Lock-free, bounded snapshot of framework-owned metric families.
#[derive(Clone, Debug)]
pub(crate) struct MetricsSnapshot {
    families: Vec<MetricFamilySnapshot>,
}

impl MetricsSnapshot {
    pub(crate) fn families(&self) -> &[MetricFamilySnapshot] {
        &self.families
    }
}

#[derive(Clone, Debug)]
pub(crate) struct MetricFamilySnapshot {
    pub(crate) family: MetricFamily,
    pub(crate) samples: Vec<MetricSample>,
}

#[derive(Clone, Debug)]
pub(crate) struct MetricSample {
    pub(crate) labels: Vec<(String, String)>,
    pub(crate) value: MetricSampleValue,
}

impl MetricSample {
    fn counter(labels: Vec<(String, String)>, value: u64) -> Self {
        Self {
            labels,
            value: MetricSampleValue::Counter(value),
        }
    }

    fn gauge(labels: Vec<(String, String)>, value: f64) -> Self {
        Self {
            labels,
            value: MetricSampleValue::Gauge(value),
        }
    }

    fn histogram(labels: Vec<(String, String)>, histogram: &Histogram) -> Self {
        let buckets = HISTOGRAM_BUCKETS
            .iter()
            .zip(histogram.bucket_counts.iter())
            .map(|(upper_bound, count)| HistogramBucketSnapshot {
                upper_bound: *upper_bound,
                count: *count,
            })
            .collect();
        Self {
            labels,
            value: MetricSampleValue::Histogram(HistogramSnapshot {
                buckets,
                sum: histogram.sum,
                count: histogram.count,
            }),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum MetricSampleValue {
    Counter(u64),
    Gauge(f64),
    Histogram(HistogramSnapshot),
}

#[derive(Clone, Debug)]
pub(crate) struct HistogramSnapshot {
    pub(crate) buckets: Vec<HistogramBucketSnapshot>,
    pub(crate) sum: f64,
    pub(crate) count: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct HistogramBucketSnapshot {
    pub(crate) upper_bound: f64,
    pub(crate) count: u64,
}

#[derive(Default)]
struct MetricsRegistry {
    service_info: Mutex<BTreeMap<String, ()>>,
    dispatch_total: Mutex<BTreeMap<DispatchCounterKey, u64>>,
    dispatch_duration: Mutex<BTreeMap<DispatchHistogramKey, Histogram>>,
    transport_messages_total: Mutex<BTreeMap<TransportMessageKey, u64>>,
    transport_failures_total: Mutex<BTreeMap<TransportFailureKey, u64>>,
    transport_publish_total: Mutex<BTreeMap<TransportPublishCounterKey, u64>>,
    transport_publish_duration: Mutex<BTreeMap<TransportPublishHistogramKey, Histogram>>,
    transport_publish_failures_total: Mutex<BTreeMap<TransportPublishFailureKey, u64>>,
    outbox_messages_total: Mutex<BTreeMap<OutboxMessageKey, u64>>,
    outbox_pending_messages: Mutex<BTreeMap<String, f64>>,
    outbox_oldest_pending_age_seconds: Mutex<BTreeMap<String, f64>>,
}

impl MetricsRegistry {
    fn describe_service(&self, service: String) {
        self.note_service(service);
    }

    fn record_microsvc_dispatch(&self, key: DispatchKey) {
        let service = key.service.clone();
        self.lock(&self.dispatch_total)
            .entry(key.counter_key())
            .and_modify(|value| *value += 1)
            .or_insert(1);
        self.lock(&self.dispatch_duration)
            .entry(key.histogram_key())
            .or_insert_with(Histogram::new)
            .observe(key.duration_seconds);
        self.note_service(service);
    }

    fn record_transport_message(&self, key: TransportMessageKey) {
        let service = key.service.clone();
        self.lock(&self.transport_messages_total)
            .entry(key)
            .and_modify(|value| *value += 1)
            .or_insert(1);
        self.note_service(service);
    }

    fn record_transport_failure(&self, key: TransportFailureKey) {
        let service = key.service.clone();
        self.lock(&self.transport_failures_total)
            .entry(key)
            .and_modify(|value| *value += 1)
            .or_insert(1);
        self.note_service(service);
    }

    fn record_transport_publish(&self, key: TransportPublishKey) {
        let service = key.service.clone();
        self.lock(&self.transport_publish_total)
            .entry(key.counter_key())
            .and_modify(|value| *value += 1)
            .or_insert(1);
        self.lock(&self.transport_publish_duration)
            .entry(key.histogram_key())
            .or_insert_with(Histogram::new)
            .observe(key.duration_seconds);
        self.note_service(service);
    }

    fn record_transport_publish_failure(&self, key: TransportPublishFailureKey) {
        let service = key.service.clone();
        self.lock(&self.transport_publish_failures_total)
            .entry(key)
            .and_modify(|value| *value += 1)
            .or_insert(1);
        self.note_service(service);
    }

    fn record_outbox_messages(&self, key: OutboxMessageKey, count: u64) {
        let service = key.service.clone();
        self.lock(&self.outbox_messages_total)
            .entry(key)
            .and_modify(|value| *value += count)
            .or_insert(count);
        self.note_service(service);
    }

    fn set_outbox_backlog(&self, service: String, pending: f64, oldest_pending_age: Option<f64>) {
        self.lock(&self.outbox_pending_messages)
            .insert(service.clone(), pending);
        if let Some(age) = oldest_pending_age {
            self.lock(&self.outbox_oldest_pending_age_seconds)
                .insert(service.clone(), age);
        } else {
            self.lock(&self.outbox_oldest_pending_age_seconds)
                .remove(&service);
        }
        self.note_service(service);
    }

    fn snapshot(&self) -> MetricsSnapshot {
        let service_info = self.clone_locked(&self.service_info);
        let dispatch_total = self.clone_locked(&self.dispatch_total);
        let dispatch_duration = self.clone_locked(&self.dispatch_duration);
        let transport_messages_total = self.clone_locked(&self.transport_messages_total);
        let transport_failures_total = self.clone_locked(&self.transport_failures_total);
        let transport_publish_total = self.clone_locked(&self.transport_publish_total);
        let transport_publish_duration = self.clone_locked(&self.transport_publish_duration);
        let transport_publish_failures_total =
            self.clone_locked(&self.transport_publish_failures_total);
        let outbox_messages_total = self.clone_locked(&self.outbox_messages_total);
        let outbox_pending_messages = self.clone_locked(&self.outbox_pending_messages);
        let outbox_oldest_pending_age_seconds =
            self.clone_locked(&self.outbox_oldest_pending_age_seconds);

        MetricsSnapshot {
            families: vec![
                SERVICE_INFO_FAMILY.snapshot(
                    service_info
                        .keys()
                        .map(|service| {
                            MetricSample::gauge(
                                vec![
                                    (metric_labels::SERVICE.to_string(), service.clone()),
                                    (
                                        metric_labels::VERSION.to_string(),
                                        env!("CARGO_PKG_VERSION").to_string(),
                                    ),
                                ],
                                1.0,
                            )
                        })
                        .collect(),
                ),
                MICROSVC_DISPATCH_TOTAL_FAMILY.snapshot(
                    dispatch_total
                        .iter()
                        .map(|(key, value)| MetricSample::counter(key.labels(), *value))
                        .collect(),
                ),
                MICROSVC_DISPATCH_DURATION_FAMILY.snapshot(
                    dispatch_duration
                        .iter()
                        .map(|(key, histogram)| MetricSample::histogram(key.labels(), histogram))
                        .collect(),
                ),
                TRANSPORT_MESSAGES_TOTAL_FAMILY.snapshot(
                    transport_messages_total
                        .iter()
                        .map(|(key, value)| MetricSample::counter(key.labels(), *value))
                        .collect(),
                ),
                TRANSPORT_FAILURES_TOTAL_FAMILY.snapshot(
                    transport_failures_total
                        .iter()
                        .map(|(key, value)| MetricSample::counter(key.labels(), *value))
                        .collect(),
                ),
                TRANSPORT_PUBLISH_TOTAL_FAMILY.snapshot(
                    transport_publish_total
                        .iter()
                        .map(|(key, value)| MetricSample::counter(key.labels(), *value))
                        .collect(),
                ),
                TRANSPORT_PUBLISH_DURATION_FAMILY.snapshot(
                    transport_publish_duration
                        .iter()
                        .map(|(key, histogram)| MetricSample::histogram(key.labels(), histogram))
                        .collect(),
                ),
                TRANSPORT_PUBLISH_FAILURES_TOTAL_FAMILY.snapshot(
                    transport_publish_failures_total
                        .iter()
                        .map(|(key, value)| MetricSample::counter(key.labels(), *value))
                        .collect(),
                ),
                OUTBOX_MESSAGES_TOTAL_FAMILY.snapshot(
                    outbox_messages_total
                        .iter()
                        .map(|(key, value)| MetricSample::counter(key.labels(), *value))
                        .collect(),
                ),
                OUTBOX_PENDING_MESSAGES_FAMILY.snapshot(
                    outbox_pending_messages
                        .iter()
                        .map(|(service, value)| {
                            MetricSample::gauge(service_labels(service), *value)
                        })
                        .collect(),
                ),
                OUTBOX_OLDEST_PENDING_AGE_FAMILY.snapshot(
                    outbox_oldest_pending_age_seconds
                        .iter()
                        .map(|(service, value)| {
                            MetricSample::gauge(service_labels(service), *value)
                        })
                        .collect(),
                ),
            ],
        }
    }

    #[cfg(test)]
    fn reset(&self) {
        self.lock(&self.service_info).clear();
        self.lock(&self.dispatch_total).clear();
        self.lock(&self.dispatch_duration).clear();
        self.lock(&self.transport_messages_total).clear();
        self.lock(&self.transport_failures_total).clear();
        self.lock(&self.transport_publish_total).clear();
        self.lock(&self.transport_publish_duration).clear();
        self.lock(&self.transport_publish_failures_total).clear();
        self.lock(&self.outbox_messages_total).clear();
        self.lock(&self.outbox_pending_messages).clear();
        self.lock(&self.outbox_oldest_pending_age_seconds).clear();
    }

    fn note_service(&self, service: String) {
        self.lock(&self.service_info).insert(service, ());
    }

    fn clone_locked<T: Clone>(&self, mutex: &Mutex<T>) -> T {
        self.lock(mutex).clone()
    }

    fn lock<'a, T>(&self, mutex: &'a Mutex<T>) -> StdMutexGuard<'a, T> {
        mutex
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
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
    fn labels(&self) -> Vec<(String, String)> {
        vec![
            (metric_labels::SERVICE.to_string(), self.service.clone()),
            (
                metric_labels::MESSAGE_KIND.to_string(),
                self.message_kind.clone(),
            ),
            (metric_labels::MESSAGE.to_string(), self.message.clone()),
            (metric_labels::STATUS.to_string(), self.status.clone()),
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
    fn labels(&self) -> Vec<(String, String)> {
        vec![
            (metric_labels::SERVICE.to_string(), self.service.clone()),
            (
                metric_labels::MESSAGE_KIND.to_string(),
                self.message_kind.clone(),
            ),
            (metric_labels::MESSAGE.to_string(), self.message.clone()),
            (metric_labels::STATUS.to_string(), self.status.clone()),
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
    fn labels(&self) -> Vec<(String, String)> {
        vec![
            (metric_labels::SERVICE.to_string(), self.service.clone()),
            (metric_labels::TRANSPORT.to_string(), self.transport.clone()),
            (
                metric_labels::MESSAGE_KIND.to_string(),
                self.message_kind.clone(),
            ),
            (metric_labels::OUTCOME.to_string(), self.outcome.clone()),
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
    fn labels(&self) -> Vec<(String, String)> {
        vec![
            (metric_labels::SERVICE.to_string(), self.service.clone()),
            (metric_labels::TRANSPORT.to_string(), self.transport.clone()),
            (
                metric_labels::FAILURE_CLASS.to_string(),
                self.failure_class.clone(),
            ),
            (metric_labels::ACTION.to_string(), self.action.clone()),
        ]
    }
}

#[derive(Clone)]
struct TransportPublishKey {
    service: String,
    transport: String,
    message_kind: String,
    outcome: String,
    duration_seconds: f64,
}

impl TransportPublishKey {
    fn counter_key(&self) -> TransportPublishCounterKey {
        TransportPublishCounterKey {
            service: self.service.clone(),
            transport: self.transport.clone(),
            message_kind: self.message_kind.clone(),
            outcome: self.outcome.clone(),
        }
    }

    fn histogram_key(&self) -> TransportPublishHistogramKey {
        TransportPublishHistogramKey {
            service: self.service.clone(),
            transport: self.transport.clone(),
            message_kind: self.message_kind.clone(),
            outcome: self.outcome.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct TransportPublishCounterKey {
    service: String,
    transport: String,
    message_kind: String,
    outcome: String,
}

impl TransportPublishCounterKey {
    fn labels(&self) -> Vec<(String, String)> {
        transport_publish_labels(
            &self.service,
            &self.transport,
            &self.message_kind,
            &self.outcome,
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct TransportPublishHistogramKey {
    service: String,
    transport: String,
    message_kind: String,
    outcome: String,
}

impl TransportPublishHistogramKey {
    fn labels(&self) -> Vec<(String, String)> {
        transport_publish_labels(
            &self.service,
            &self.transport,
            &self.message_kind,
            &self.outcome,
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct TransportPublishFailureKey {
    service: String,
    transport: String,
    message_kind: String,
    failure_class: String,
}

impl TransportPublishFailureKey {
    fn labels(&self) -> Vec<(String, String)> {
        vec![
            (metric_labels::SERVICE.to_string(), self.service.clone()),
            (metric_labels::TRANSPORT.to_string(), self.transport.clone()),
            (
                metric_labels::MESSAGE_KIND.to_string(),
                self.message_kind.clone(),
            ),
            (
                metric_labels::FAILURE_CLASS.to_string(),
                self.failure_class.clone(),
            ),
        ]
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct OutboxMessageKey {
    service: String,
    outcome: String,
}

impl OutboxMessageKey {
    fn labels(&self) -> Vec<(String, String)> {
        vec![
            (metric_labels::SERVICE.to_string(), self.service.clone()),
            (metric_labels::OUTCOME.to_string(), self.outcome.clone()),
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

fn service_labels(service: &str) -> Vec<(String, String)> {
    vec![(metric_labels::SERVICE.to_string(), service.to_string())]
}

fn transport_publish_labels(
    service: &str,
    transport: &str,
    message_kind: &str,
    outcome: &str,
) -> Vec<(String, String)> {
    vec![
        (metric_labels::SERVICE.to_string(), service.to_string()),
        (metric_labels::TRANSPORT.to_string(), transport.to_string()),
        (
            metric_labels::MESSAGE_KIND.to_string(),
            message_kind.to_string(),
        ),
        (metric_labels::OUTCOME.to_string(), outcome.to_string()),
    ]
}

fn render_prometheus(snapshot: &MetricsSnapshot) -> String {
    let mut output = String::new();
    for family_snapshot in snapshot.families() {
        write_family_header(&mut output, family_snapshot.family);
        for sample in &family_snapshot.samples {
            match &sample.value {
                MetricSampleValue::Counter(value) => {
                    push_metric(
                        &mut output,
                        family_snapshot.family.name,
                        &sample.labels,
                        &value.to_string(),
                    );
                }
                MetricSampleValue::Gauge(value) => {
                    push_metric(
                        &mut output,
                        family_snapshot.family.name,
                        &sample.labels,
                        &format_float(*value),
                    );
                }
                MetricSampleValue::Histogram(histogram) => {
                    let bucket_name = format!("{}_bucket", family_snapshot.family.name);
                    for bucket in &histogram.buckets {
                        let mut labels = sample.labels.clone();
                        labels.push((
                            metric_labels::LE.to_string(),
                            bucket.upper_bound.to_string(),
                        ));
                        push_metric(
                            &mut output,
                            &bucket_name,
                            &labels,
                            &bucket.count.to_string(),
                        );
                    }
                    let mut labels = sample.labels.clone();
                    labels.push((metric_labels::LE.to_string(), "+Inf".to_string()));
                    push_metric(
                        &mut output,
                        &bucket_name,
                        &labels,
                        &histogram.count.to_string(),
                    );
                    push_metric(
                        &mut output,
                        &format!("{}_sum", family_snapshot.family.name),
                        &sample.labels,
                        &format_float(histogram.sum),
                    );
                    push_metric(
                        &mut output,
                        &format!("{}_count", family_snapshot.family.name),
                        &sample.labels,
                        &histogram.count.to_string(),
                    );
                }
            }
        }
    }
    output.push_str("# EOF\n");
    output
}

fn write_family_header(output: &mut String, family: MetricFamily) {
    output.push_str("# HELP ");
    output.push_str(family.name);
    output.push(' ');
    output.push_str(family.help);
    output.push('\n');
    output.push_str("# TYPE ");
    output.push_str(family.name);
    output.push(' ');
    output.push_str(family.kind.as_prometheus_type());
    output.push('\n');
}

fn push_metric(output: &mut String, name: &str, labels: &[(String, String)], value: &str) {
    output.push_str(name);
    if !labels.is_empty() {
        output.push('{');
        for (index, (key, value)) in labels.iter().enumerate() {
            if index > 0 {
                output.push(',');
            }
            output.push_str(key.as_str());
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
    use crate::telemetry::{
        dispatch_status, failure_action, failure_class, metric_labels, metric_names,
        outbox_outcome, transport_outcome, transport_publish_outcome,
    };
    use std::collections::BTreeSet;

    #[test]
    fn prometheus_text_escapes_label_values() {
        let _guard = lock_for_tests();
        reset_for_tests();

        record_microsvc_dispatch(
            Some("svc\"one"),
            MessageKind::Command,
            "line\nbreak",
            dispatch_status::SUCCESS,
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
            dispatch_status::SUCCESS,
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

    #[test]
    fn snapshot_exposes_bounded_family_shape() {
        let _guard = lock_for_tests();
        reset_for_tests();

        record_microsvc_dispatch(
            Some("orders"),
            MessageKind::Event,
            "orders.created",
            dispatch_status::SUCCESS,
            Duration::from_millis(7),
        );
        record_transport_message(
            Some("orders"),
            "nats",
            MessageKind::Event,
            transport_outcome::ACK,
        );
        record_transport_failure(
            Some("orders"),
            "nats",
            failure_class::RETRYABLE,
            failure_action::NACK,
        );
        record_transport_publish(
            Some("orders"),
            "nats",
            MessageKind::Event,
            transport_publish_outcome::PUBLISHED,
            Duration::from_millis(9),
        );
        record_transport_publish_failure(
            Some("orders"),
            "nats",
            MessageKind::Command,
            failure_class::PERMANENT,
        );
        record_outbox_messages(Some("orders"), outbox_outcome::PUBLISHED, 2);
        set_outbox_backlog(Some("orders"), 3, Some(Duration::from_secs(4)));

        let snapshot = snapshot();
        let family_names = snapshot
            .families()
            .iter()
            .map(|family| family.family.name)
            .collect::<Vec<_>>();

        assert_eq!(
            family_names,
            vec![
                metric_names::SERVICE_INFO,
                metric_names::MICROSVC_DISPATCH_TOTAL,
                metric_names::MICROSVC_DISPATCH_DURATION_SECONDS,
                metric_names::TRANSPORT_MESSAGES_TOTAL,
                metric_names::TRANSPORT_FAILURES_TOTAL,
                metric_names::TRANSPORT_PUBLISH_TOTAL,
                metric_names::TRANSPORT_PUBLISH_DURATION_SECONDS,
                metric_names::TRANSPORT_PUBLISH_FAILURES_TOTAL,
                metric_names::OUTBOX_MESSAGES_TOTAL,
                metric_names::OUTBOX_PENDING_MESSAGES,
                metric_names::OUTBOX_OLDEST_PENDING_AGE_SECONDS,
            ]
        );

        let dispatch_family = snapshot
            .families()
            .iter()
            .find(|family| family.family.name == metric_names::MICROSVC_DISPATCH_TOTAL)
            .expect("dispatch family is present");
        let dispatch_sample = dispatch_family
            .samples
            .iter()
            .find(|sample| {
                label_value(&sample.labels, metric_labels::MESSAGE) == Some("orders.created")
            })
            .expect("dispatch sample is present");
        assert!(matches!(
            &dispatch_sample.value,
            MetricSampleValue::Counter(value) if *value == 1
        ));

        let duration_family = snapshot
            .families()
            .iter()
            .find(|family| family.family.name == metric_names::MICROSVC_DISPATCH_DURATION_SECONDS)
            .expect("duration family is present");
        assert!(matches!(
            &duration_family.samples[0].value,
            MetricSampleValue::Histogram(_)
        ));
    }

    #[test]
    fn rendered_metrics_use_only_allowed_low_cardinality_labels() {
        let _guard = lock_for_tests();
        reset_for_tests();

        describe_service(Some("orders"));
        record_microsvc_dispatch(
            Some("orders"),
            MessageKind::Command,
            "orders.create",
            dispatch_status::SUCCESS,
            Duration::from_millis(2),
        );
        record_transport_message(
            Some("orders"),
            "rabbitmq",
            MessageKind::Command,
            transport_outcome::ACK,
        );
        record_transport_failure(
            Some("orders"),
            "rabbitmq",
            failure_class::PERMANENT,
            failure_action::DEAD_LETTER,
        );
        record_transport_publish(
            Some("orders"),
            "rabbitmq",
            MessageKind::Command,
            transport_publish_outcome::FAILED,
            Duration::from_millis(5),
        );
        record_transport_publish_failure(
            Some("orders"),
            "rabbitmq",
            MessageKind::Command,
            failure_class::PERMANENT,
        );
        record_outbox_messages(Some("orders"), outbox_outcome::RELEASED, 1);
        set_outbox_backlog(Some("orders"), 1, Some(Duration::from_secs(30)));

        let text = prometheus_text();
        let label_names = rendered_label_names(&text);

        for forbidden in crate::telemetry::privacy_policy::FORBIDDEN_METRIC_LABELS {
            assert!(
                !label_names.contains(*forbidden),
                "forbidden label `{forbidden}` appeared in metrics:\n{text}"
            );
        }
        for label in &label_names {
            assert!(
                crate::telemetry::privacy_policy::ALLOWED_METRIC_LABELS.contains(&label.as_str()),
                "unexpected metric label `{label}` appeared in metrics:\n{text}"
            );
        }
    }

    fn label_value<'a>(labels: &'a [(String, String)], name: &str) -> Option<&'a str> {
        labels
            .iter()
            .find(|(label, _)| label == name)
            .map(|(_, value)| value.as_str())
    }

    fn rendered_label_names(text: &str) -> BTreeSet<String> {
        let mut names = BTreeSet::new();
        for line in text.lines().filter(|line| !line.starts_with('#')) {
            let Some(open) = line.find('{') else {
                continue;
            };
            let Some(close_offset) = line[open + 1..].find('}') else {
                continue;
            };
            let labels = &line[open + 1..open + 1 + close_offset];
            let bytes = labels.as_bytes();
            let mut index = 0;
            while index < bytes.len() {
                let name_start = index;
                while index < bytes.len() && bytes[index] != b'=' {
                    index += 1;
                }
                if index >= bytes.len() {
                    break;
                }
                names.insert(labels[name_start..index].to_string());
                index += 1;

                if bytes.get(index) == Some(&b'"') {
                    index += 1;
                }
                while index < bytes.len() {
                    match bytes[index] {
                        b'\\' => index = (index + 2).min(bytes.len()),
                        b'"' => {
                            index += 1;
                            break;
                        }
                        _ => index += 1,
                    }
                }
                if bytes.get(index) == Some(&b',') {
                    index += 1;
                }
            }
        }
        names
    }
}
