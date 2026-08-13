use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard as StdMutexGuard, OnceLock};

use crate::telemetry::{metric_labels, metric_names};

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
const GRAPHQL_REQUEST_TOTAL_FAMILY: MetricFamily = MetricFamily::counter(
    metric_names::GRAPHQL_REQUEST_TOTAL,
    "Total GraphQL root-field executions by service, root_field, and status.",
);
const GRAPHQL_REQUEST_DURATION_FAMILY: MetricFamily = MetricFamily::histogram(
    metric_names::GRAPHQL_REQUEST_DURATION_SECONDS,
    "GraphQL root-field execution duration in seconds.",
);

static REGISTRY: OnceLock<MetricsRegistry> = OnceLock::new();

pub(super) fn registry() -> &'static MetricsRegistry {
    REGISTRY.get_or_init(MetricsRegistry::default)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MetricFamily {
    pub(crate) name: &'static str,
    pub(super) help: &'static str,
    pub(super) kind: MetricKind,
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

    pub(super) fn snapshot(self, samples: Vec<MetricSample>) -> MetricFamilySnapshot {
        MetricFamilySnapshot {
            family: self,
            samples,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum MetricKind {
    Counter,
    Gauge,
    Histogram,
}

impl MetricKind {
    pub(super) fn as_prometheus_type(self) -> &'static str {
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
pub(super) struct MetricsRegistry {
    service_info: Mutex<BTreeMap<String, ()>>,
    dispatch_total: Mutex<BTreeMap<DispatchCounterKey, u64>>,
    dispatch_duration: Mutex<BTreeMap<DispatchHistogramKey, Histogram>>,
    transport_messages_total: Mutex<BTreeMap<TransportMessageKey, u64>>,
    transport_failures_total: Mutex<BTreeMap<TransportFailureKey, u64>>,
    outbox_messages_total: Mutex<BTreeMap<OutboxMessageKey, u64>>,
    outbox_pending_messages: Mutex<BTreeMap<String, f64>>,
    outbox_oldest_pending_age_seconds: Mutex<BTreeMap<String, f64>>,
    graphql_request_total: Mutex<BTreeMap<GraphqlCounterKey, u64>>,
    graphql_request_duration: Mutex<BTreeMap<GraphqlHistogramKey, Histogram>>,
}

impl MetricsRegistry {
    pub(super) fn describe_service(&self, service: String) {
        self.note_service(service);
    }

    pub(super) fn record_microsvc_dispatch(&self, key: DispatchKey) {
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

    pub(super) fn record_transport_message(&self, key: TransportMessageKey) {
        let service = key.service.clone();
        self.lock(&self.transport_messages_total)
            .entry(key)
            .and_modify(|value| *value += 1)
            .or_insert(1);
        self.note_service(service);
    }

    pub(super) fn record_transport_failure(&self, key: TransportFailureKey) {
        let service = key.service.clone();
        self.lock(&self.transport_failures_total)
            .entry(key)
            .and_modify(|value| *value += 1)
            .or_insert(1);
        self.note_service(service);
    }

    pub(super) fn record_outbox_messages(&self, key: OutboxMessageKey, count: u64) {
        let service = key.service.clone();
        self.lock(&self.outbox_messages_total)
            .entry(key)
            .and_modify(|value| *value += count)
            .or_insert(count);
        self.note_service(service);
    }

    pub(super) fn record_graphql_request(&self, key: GraphqlRequestKey) {
        let service = key.service.clone();
        self.lock(&self.graphql_request_total)
            .entry(key.counter_key())
            .and_modify(|value| *value += 1)
            .or_insert(1);
        self.lock(&self.graphql_request_duration)
            .entry(key.histogram_key())
            .or_insert_with(Histogram::new)
            .observe(key.duration_seconds);
        self.note_service(service);
    }

    pub(super) fn set_outbox_backlog(
        &self,
        service: String,
        pending: f64,
        oldest_pending_age: Option<f64>,
    ) {
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

    pub(super) fn snapshot(&self) -> MetricsSnapshot {
        let service_info = self.clone_locked(&self.service_info);
        let dispatch_total = self.clone_locked(&self.dispatch_total);
        let dispatch_duration = self.clone_locked(&self.dispatch_duration);
        let transport_messages_total = self.clone_locked(&self.transport_messages_total);
        let transport_failures_total = self.clone_locked(&self.transport_failures_total);
        let outbox_messages_total = self.clone_locked(&self.outbox_messages_total);
        let outbox_pending_messages = self.clone_locked(&self.outbox_pending_messages);
        let outbox_oldest_pending_age_seconds =
            self.clone_locked(&self.outbox_oldest_pending_age_seconds);
        let graphql_request_total = self.clone_locked(&self.graphql_request_total);
        let graphql_request_duration = self.clone_locked(&self.graphql_request_duration);

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
                GRAPHQL_REQUEST_TOTAL_FAMILY.snapshot(
                    graphql_request_total
                        .iter()
                        .map(|(key, value)| MetricSample::counter(key.labels(), *value))
                        .collect(),
                ),
                GRAPHQL_REQUEST_DURATION_FAMILY.snapshot(
                    graphql_request_duration
                        .iter()
                        .map(|(key, histogram)| MetricSample::histogram(key.labels(), histogram))
                        .collect(),
                ),
            ],
        }
    }

    #[cfg(test)]
    pub(super) fn reset(&self) {
        self.lock(&self.service_info).clear();
        self.lock(&self.dispatch_total).clear();
        self.lock(&self.dispatch_duration).clear();
        self.lock(&self.transport_messages_total).clear();
        self.lock(&self.transport_failures_total).clear();
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
pub(super) struct DispatchKey {
    pub(super) service: String,
    pub(super) message_kind: String,
    pub(super) message: String,
    pub(super) status: String,
    pub(super) duration_seconds: f64,
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
pub(super) struct TransportMessageKey {
    pub(super) service: String,
    pub(super) transport: String,
    pub(super) message_kind: String,
    pub(super) outcome: String,
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
pub(super) struct TransportFailureKey {
    pub(super) service: String,
    pub(super) transport: String,
    pub(super) failure_class: String,
    pub(super) action: String,
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

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct OutboxMessageKey {
    pub(super) service: String,
    pub(super) outcome: String,
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
#[derive(Clone, Debug)]
pub(super) struct GraphqlRequestKey {
    pub(super) service: String,
    pub(super) root_field: String,
    pub(super) status: String,
    pub(super) duration_seconds: f64,
}

impl GraphqlRequestKey {
    fn counter_key(&self) -> GraphqlCounterKey {
        GraphqlCounterKey {
            service: self.service.clone(),
            root_field: self.root_field.clone(),
            status: self.status.clone(),
        }
    }
    fn histogram_key(&self) -> GraphqlHistogramKey {
        GraphqlHistogramKey {
            service: self.service.clone(),
            root_field: self.root_field.clone(),
            status: self.status.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct GraphqlCounterKey {
    service: String,
    root_field: String,
    status: String,
}

impl GraphqlCounterKey {
    fn labels(&self) -> Vec<(String, String)> {
        vec![
            (metric_labels::SERVICE.to_string(), self.service.clone()),
            ("root_field".to_string(), self.root_field.clone()),
            (metric_labels::STATUS.to_string(), self.status.clone()),
        ]
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct GraphqlHistogramKey {
    service: String,
    root_field: String,
    status: String,
}

impl GraphqlHistogramKey {
    fn labels(&self) -> Vec<(String, String)> {
        vec![
            (metric_labels::SERVICE.to_string(), self.service.clone()),
            ("root_field".to_string(), self.root_field.clone()),
            (metric_labels::STATUS.to_string(), self.status.clone()),
        ]
    }
}
