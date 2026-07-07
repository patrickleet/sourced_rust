//! Private diagnostics snapshots for operators and agents.
//!
//! Diagnostics are not installed into any HTTP router by default. A service must
//! explicitly compose the diagnostics route, and deployments must keep it behind
//! a private listener, trusted proxy, mTLS, or equivalent access control.

use std::collections::{BTreeSet, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard as StdMutexGuard, OnceLock};
use std::time::{Duration, SystemTime};

#[cfg(feature = "http")]
use axum::http::{header, HeaderMap};
use serde::{Deserialize, Serialize};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::bus::{Message, MessageKind, TransportErrorKind};
use crate::microsvc::{DeliveryKind, HandlerError, HandlerSpec, Service};
use crate::outbox_worker::{OutboxBacklogStats, OutboxStore, BACKLOG_STATS_SCAN_LIMIT};
use crate::repository::RepositoryError;
use crate::trace_context::is_valid_traceparent;

pub const DIAGNOSTICS_SCHEMA_VERSION: u32 = 1;
pub const DEFAULT_DIAGNOSTICS_PATH: &str = "/_distributed/diagnostics";
const DEFAULT_TTL: Duration = Duration::from_secs(1);
const DEFAULT_RECENT_FAILURE_CAPACITY: usize = 100;
const DEFAULT_RECENT_FAILURE_LIMIT: usize = 32;
const DEFAULT_COMMAND_LIMIT: usize = 64;
const DEFAULT_EVENT_LIMIT: usize = 64;
const DEFAULT_HANDLER_LIMIT: usize = 64;
const DEFAULT_TRANSPORT_LIMIT: usize = 16;
const DEFAULT_METRIC_FAMILY_LIMIT: usize = 64;
const DEFAULT_STRING_LIMIT: usize = 512;
const DEFAULT_ID_LIMIT: usize = 128;
const DEFAULT_RESPONSE_SIZE_LIMIT: usize = 64 * 1024;

type BacklogStatsFuture =
    Pin<Box<dyn Future<Output = Result<OutboxBacklogStats, RepositoryError>> + Send>>;
type BacklogStatsProvider = Arc<dyn Fn() -> BacklogStatsFuture + Send + Sync>;
#[cfg(feature = "http")]
type AccessCheck = Arc<dyn Fn(&HeaderMap) -> bool + Send + Sync>;

static RECENT_FAILURES: OnceLock<Mutex<RecentFailureRingBuffer>> = OnceLock::new();

#[derive(Clone)]
pub struct Diagnostics {
    inner: Arc<DiagnosticsInner>,
}

struct DiagnosticsInner {
    options: DiagnosticsOptions,
    cache: Mutex<Option<CachedSnapshot>>,
}

#[derive(Clone)]
struct CachedSnapshot {
    generated_at: SystemTime,
    snapshot: DiagnosticsSnapshot,
}

#[derive(Clone)]
pub struct DiagnosticsOptions {
    ttl: Duration,
    command_limit: usize,
    event_limit: usize,
    handler_limit: usize,
    transport_limit: usize,
    metric_family_limit: usize,
    recent_failure_limit: usize,
    string_limit: usize,
    id_limit: usize,
    max_response_bytes: usize,
    instance_id: Option<String>,
    transports: Vec<String>,
    diagnostics_path: String,
    backlog_stats: Option<BacklogStatsProvider>,
    #[cfg(feature = "http")]
    access_check: Option<AccessCheck>,
}

impl Default for DiagnosticsOptions {
    fn default() -> Self {
        Self {
            ttl: DEFAULT_TTL,
            command_limit: DEFAULT_COMMAND_LIMIT,
            event_limit: DEFAULT_EVENT_LIMIT,
            handler_limit: DEFAULT_HANDLER_LIMIT,
            transport_limit: DEFAULT_TRANSPORT_LIMIT,
            metric_family_limit: DEFAULT_METRIC_FAMILY_LIMIT,
            recent_failure_limit: DEFAULT_RECENT_FAILURE_LIMIT,
            string_limit: DEFAULT_STRING_LIMIT,
            id_limit: DEFAULT_ID_LIMIT,
            max_response_bytes: DEFAULT_RESPONSE_SIZE_LIMIT,
            instance_id: None,
            transports: Vec::new(),
            diagnostics_path: DEFAULT_DIAGNOSTICS_PATH.to_string(),
            backlog_stats: None,
            #[cfg(feature = "http")]
            access_check: None,
        }
    }
}

impl DiagnosticsOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_ttl(mut self, ttl: Duration) -> Self {
        self.ttl = ttl;
        self
    }

    pub fn with_command_limit(mut self, limit: usize) -> Self {
        self.command_limit = limit;
        self
    }

    pub fn with_event_limit(mut self, limit: usize) -> Self {
        self.event_limit = limit;
        self
    }

    pub fn with_recent_failure_limit(mut self, limit: usize) -> Self {
        self.recent_failure_limit = limit;
        self
    }

    pub fn with_string_limit(mut self, limit: usize) -> Self {
        self.string_limit = limit;
        self
    }

    pub fn with_max_response_bytes(mut self, limit: usize) -> Self {
        self.max_response_bytes = limit;
        self
    }

    pub fn with_instance_id(mut self, instance_id: impl Into<String>) -> Self {
        self.instance_id = Some(instance_id.into());
        self
    }

    pub fn with_transport(mut self, transport: impl Into<String>) -> Self {
        self.transports.push(transport.into());
        self
    }

    pub fn with_diagnostics_path(mut self, path: impl Into<String>) -> Self {
        self.diagnostics_path = path.into();
        self
    }

    #[cfg(feature = "http")]
    pub(crate) fn diagnostics_path(&self) -> &str {
        &self.diagnostics_path
    }

    pub fn with_outbox_store<S>(self, store: S) -> Self
    where
        S: OutboxStore + 'static,
    {
        let store = Arc::new(store);
        self.with_outbox_backlog_provider(move || {
            let store = Arc::clone(&store);
            async move { store.backlog_stats().await }
        })
    }

    pub fn with_outbox_backlog_provider<F, Fut>(mut self, provider: F) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<OutboxBacklogStats, RepositoryError>> + Send + 'static,
    {
        self.backlog_stats = Some(Arc::new(move || Box::pin(provider())));
        self
    }

    #[cfg(feature = "http")]
    pub fn with_access_check<F>(mut self, access_check: F) -> Self
    where
        F: Fn(&HeaderMap) -> bool + Send + Sync + 'static,
    {
        self.access_check = Some(Arc::new(access_check));
        self
    }

    #[cfg(feature = "http")]
    pub fn with_bearer_token(self, token: impl Into<String>) -> Self {
        let expected = format!("Bearer {}", token.into());
        self.with_access_check(move |headers| {
            headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value == expected)
        })
    }
}

impl Diagnostics {
    pub fn new(options: DiagnosticsOptions) -> Self {
        Self {
            inner: Arc::new(DiagnosticsInner {
                options,
                cache: Mutex::new(None),
            }),
        }
    }

    pub async fn snapshot(&self, service: &Service) -> DiagnosticsSnapshot {
        let now = SystemTime::now();
        if let Some(snapshot) = self.cached_snapshot(now) {
            return snapshot;
        }

        let snapshot = build_snapshot(service, &self.inner.options, now).await;
        *self.lock_cache() = Some(CachedSnapshot {
            generated_at: now,
            snapshot: snapshot.clone(),
        });
        snapshot
    }

    pub fn invalidate(&self) {
        *self.lock_cache() = None;
    }

    #[cfg(feature = "http")]
    pub(crate) fn authorized(&self, headers: &HeaderMap) -> bool {
        self.inner
            .options
            .access_check
            .as_ref()
            .map(|check| check(headers))
            .unwrap_or(true)
    }

    fn cached_snapshot(&self, now: SystemTime) -> Option<DiagnosticsSnapshot> {
        let cached = self.lock_cache().clone()?;
        let age = now.duration_since(cached.generated_at).unwrap_or_default();
        if age > self.inner.options.ttl {
            return None;
        }
        Some(with_age(cached.snapshot, cached.generated_at, now))
    }

    fn lock_cache(&self) -> StdMutexGuard<'_, Option<CachedSnapshot>> {
        self.inner
            .cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl Default for Diagnostics {
    fn default() -> Self {
        Self::new(DiagnosticsOptions::default())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DiagnosticsSnapshot {
    pub schema_version: u32,
    pub generated_at: String,
    pub snapshot: SnapshotMetadata,
    pub service: ServiceDiagnostics,
    pub health: HealthDiagnostics,
    pub telemetry: TelemetryDiagnostics,
    pub backlogs: BacklogDiagnostics,
    pub recent_failures: Vec<RecentFailure>,
    pub recent_failures_meta: RecentFailuresMetadata,
    pub causal_hints: CausalHints,
    pub actions: Vec<SuggestedAction>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotMetadata {
    pub age_ms: u64,
    pub ttl_ms: u64,
    pub partial: bool,
    pub truncated: bool,
    pub partial_errors: Vec<PartialError>,
    pub limits: SnapshotLimits,
    pub response_size_bytes: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotLimits {
    pub max_response_bytes: usize,
    pub string_bytes: usize,
    pub id_bytes: usize,
    pub commands: usize,
    pub events: usize,
    pub handlers: usize,
    pub transports: usize,
    pub recent_failures: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartialError {
    pub section: String,
    pub error_summary: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceDiagnostics {
    pub name: String,
    pub version: String,
    pub instance_id: Option<String>,
    pub commands: Vec<String>,
    pub commands_meta: ListMetadata,
    pub events: Vec<String>,
    pub events_meta: ListMetadata,
    pub handler_specs: Vec<HandlerSpecDiagnostic>,
    pub handler_specs_meta: ListMetadata,
    pub subscription_plan: SubscriptionPlanDiagnostics,
    pub transports: Vec<String>,
    pub transports_meta: ListMetadata,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandlerSpecDiagnostic {
    pub names: Vec<String>,
    pub names_meta: ListMetadata,
    pub kind: String,
    pub delivery: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscriptionPlanDiagnostics {
    pub commands: Vec<String>,
    pub commands_meta: ListMetadata,
    pub events: Vec<String>,
    pub events_meta: ListMetadata,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListMetadata {
    pub limit: usize,
    pub omitted_count: usize,
    pub truncated: bool,
}

impl ListMetadata {
    fn new(limit: usize, total: usize, returned: usize) -> Self {
        let omitted_count = total.saturating_sub(returned);
        Self {
            limit,
            omitted_count,
            truncated: omitted_count > 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthDiagnostics {
    pub status: String,
    pub readiness: ReadinessDiagnostics,
    pub checks: Vec<HealthCheck>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadinessDiagnostics {
    pub accepts_commands: bool,
    pub consumes_bus: bool,
    pub publishes_outbox: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthCheck {
    pub id: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TelemetryDiagnostics {
    pub metrics: MetricsTelemetry,
    pub tracing: TracingTelemetry,
    pub diagnostics: DiagnosticsTelemetry,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetricsTelemetry {
    pub enabled: bool,
    pub path: Option<String>,
    pub families: Vec<String>,
    pub families_meta: ListMetadata,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TracingTelemetry {
    pub propagation: String,
    pub otel_spans: bool,
    pub export: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticsTelemetry {
    pub enabled: bool,
    pub path: String,
    pub visibility: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BacklogDiagnostics {
    pub outbox: OutboxBacklogDiagnostics,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OutboxBacklogDiagnostics {
    pub configured: bool,
    pub pending: Option<usize>,
    pub oldest_pending_age_seconds: Option<f64>,
    pub scan_limit: usize,
    pub truncated: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentFailure {
    pub at: String,
    pub service: String,
    pub kind: String,
    pub component: String,
    pub operation: String,
    pub class: String,
    pub action: String,
    pub transport: Option<String>,
    pub message_kind: Option<String>,
    pub message: String,
    pub error_category: String,
    pub error_summary: String,
    pub correlation_id: Option<String>,
    pub causation_id: Option<String>,
    pub trace_id: Option<String>,
    pub parent_span_id: Option<String>,
    pub trace_flags: Option<String>,
    pub runbook_keys: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentFailuresMetadata {
    pub capacity: usize,
    pub dropped_count: u64,
    pub limit: usize,
    pub omitted_count: usize,
    pub truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CausalHints {
    pub last_trace_ids: Vec<String>,
    pub last_correlation_ids: Vec<String>,
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuggestedAction {
    pub id: String,
    pub severity: String,
    pub evidence: Vec<String>,
    pub suggested_next_steps: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FailureMessageContext {
    name: String,
    kind: MessageKind,
    correlation_id: Option<String>,
    causation_id: Option<String>,
    traceparent: Option<String>,
}

impl FailureMessageContext {
    pub(crate) fn new(name: impl Into<String>, kind: MessageKind) -> Self {
        Self {
            name: name.into(),
            kind,
            correlation_id: None,
            causation_id: None,
            traceparent: None,
        }
    }

    pub(crate) fn from_message(message: &Message) -> Self {
        Self {
            name: message.name().to_string(),
            kind: message.kind,
            correlation_id: message.correlation_id().map(str::to_string),
            causation_id: message.causation_id().map(str::to_string),
            traceparent: message.traceparent().map(str::to_string),
        }
    }
}

#[derive(Clone, Debug)]
pub struct RecentFailureRingBuffer {
    capacity: usize,
    records: VecDeque<RecentFailure>,
    dropped_count: u64,
}

impl RecentFailureRingBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            records: VecDeque::with_capacity(capacity),
            dropped_count: 0,
        }
    }

    pub fn push(&mut self, failure: RecentFailure) {
        if self.capacity == 0 {
            self.dropped_count = self.dropped_count.saturating_add(1);
            return;
        }
        if self.records.len() >= self.capacity {
            self.records.pop_front();
            self.dropped_count = self.dropped_count.saturating_add(1);
        }
        self.records.push_back(failure);
    }

    pub fn snapshot_for_service(
        &self,
        service: &str,
        limit: usize,
    ) -> (Vec<RecentFailure>, RecentFailuresMetadata) {
        let matching = self
            .records
            .iter()
            .rev()
            .filter(|failure| failure.service == service)
            .cloned()
            .collect::<Vec<_>>();
        let mut returned = matching.into_iter().take(limit).collect::<Vec<_>>();
        returned.reverse();
        let total_for_service = self
            .records
            .iter()
            .filter(|failure| failure.service == service)
            .count();
        let omitted_count = total_for_service.saturating_sub(returned.len());
        (
            returned,
            RecentFailuresMetadata {
                capacity: self.capacity,
                dropped_count: self.dropped_count,
                limit,
                omitted_count,
                truncated: omitted_count > 0,
            },
        )
    }
}

impl Default for RecentFailureRingBuffer {
    fn default() -> Self {
        Self::new(DEFAULT_RECENT_FAILURE_CAPACITY)
    }
}

pub(crate) fn record_microsvc_failure(
    service: Option<&str>,
    message: &FailureMessageContext,
    error: &HandlerError,
) {
    let action = format!("return_{}", error.status_code());
    let class = transport_error_class(error.transport_error_kind());
    let category = handler_error_category(error);
    push_recent_failure(build_failure_record(FailureRecordInput {
        service,
        kind: "microsvc",
        component: "microsvc",
        operation: "dispatch",
        class,
        action: &action,
        transport: None,
        message: Some(message),
        error_category: category,
        error_summary: &error.to_string(),
    }));
}

pub(crate) fn record_transport_failure(
    service: Option<&str>,
    transport: &str,
    message: Option<&FailureMessageContext>,
    kind: TransportErrorKind,
    action: &str,
    error_summary: Option<&str>,
) {
    push_recent_failure(build_failure_record(FailureRecordInput {
        service,
        kind: "transport",
        component: "transport",
        operation: "receive",
        class: transport_error_class(kind),
        action,
        transport: Some(transport),
        message,
        error_category: "transport",
        error_summary: error_summary.unwrap_or(action),
    }));
}

pub(crate) fn record_outbox_publish_failure(
    service: Option<&str>,
    message: &FailureMessageContext,
    kind: TransportErrorKind,
    action: &str,
    error_summary: &str,
) {
    push_recent_failure(build_failure_record(FailureRecordInput {
        service,
        kind: "outbox",
        component: "outbox",
        operation: "publish",
        class: transport_error_class(kind),
        action,
        transport: Some("outbox"),
        message: Some(message),
        error_category: "outbox_publish",
        error_summary,
    }));
}

struct FailureRecordInput<'a> {
    service: Option<&'a str>,
    kind: &'static str,
    component: &'static str,
    operation: &'static str,
    class: &'static str,
    action: &'a str,
    transport: Option<&'a str>,
    message: Option<&'a FailureMessageContext>,
    error_category: &'static str,
    error_summary: &'a str,
}

fn build_failure_record(input: FailureRecordInput<'_>) -> RecentFailure {
    let message = input
        .message
        .map(|message| sanitize_label(&message.name, DEFAULT_ID_LIMIT))
        .unwrap_or_else(|| "unknown".to_string());
    let message_kind = input
        .message
        .map(|message| message.kind.as_str().to_string());
    let traceparent = input
        .message
        .and_then(|message| message.traceparent.as_deref());
    let trace = traceparent.and_then(parse_traceparent);
    let category = input.error_category.to_string();
    RecentFailure {
        at: format_timestamp(SystemTime::now()),
        service: service_label(input.service),
        kind: input.kind.to_string(),
        component: input.component.to_string(),
        operation: input.operation.to_string(),
        class: input.class.to_string(),
        action: sanitize_label(input.action, DEFAULT_ID_LIMIT),
        transport: input
            .transport
            .map(|transport| sanitize_label(transport, DEFAULT_ID_LIMIT)),
        message_kind,
        message,
        error_category: category.clone(),
        error_summary: sanitize_summary(input.error_summary, DEFAULT_STRING_LIMIT),
        correlation_id: input
            .message
            .and_then(|message| message.correlation_id.as_deref())
            .map(|id| sanitize_id(id, DEFAULT_ID_LIMIT)),
        causation_id: input
            .message
            .and_then(|message| message.causation_id.as_deref())
            .map(|id| sanitize_id(id, DEFAULT_ID_LIMIT)),
        trace_id: trace.as_ref().map(|trace| trace.trace_id.clone()),
        parent_span_id: trace.as_ref().map(|trace| trace.parent_span_id.clone()),
        trace_flags: trace.as_ref().map(|trace| trace.flags.clone()),
        runbook_keys: runbook_keys_for(&category, input.action, input.kind),
    }
}

fn push_recent_failure(failure: RecentFailure) {
    recent_failures()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push(failure);
}

async fn build_snapshot(
    service: &Service,
    options: &DiagnosticsOptions,
    generated_at: SystemTime,
) -> DiagnosticsSnapshot {
    let service_name = service_label(service.name());
    let service_inventory = service_diagnostics(service, options);
    let (backlogs, partial_errors) = backlog_diagnostics(options).await;
    let (recent_failures, recent_failures_meta) =
        recent_failure_snapshot(&service_name, options.recent_failure_limit);
    let causal_hints = causal_hints(&recent_failures);
    let telemetry = telemetry_diagnostics(options);
    let health = health_diagnostics(&service_inventory, &backlogs, &partial_errors);
    let actions = suggested_actions(&backlogs, &recent_failures);
    let truncated = service_inventory.commands_meta.truncated
        || service_inventory.events_meta.truncated
        || service_inventory.handler_specs_meta.truncated
        || service_inventory.transports_meta.truncated
        || telemetry.metrics.families_meta.truncated
        || recent_failures_meta.truncated
        || backlogs.outbox.truncated;
    let mut snapshot = DiagnosticsSnapshot {
        schema_version: DIAGNOSTICS_SCHEMA_VERSION,
        generated_at: format_timestamp(generated_at),
        snapshot: SnapshotMetadata {
            age_ms: 0,
            ttl_ms: duration_ms(options.ttl),
            partial: !partial_errors.is_empty(),
            truncated,
            partial_errors,
            limits: SnapshotLimits {
                max_response_bytes: options.max_response_bytes,
                string_bytes: options.string_limit,
                id_bytes: options.id_limit,
                commands: options.command_limit,
                events: options.event_limit,
                handlers: options.handler_limit,
                transports: options.transport_limit,
                recent_failures: options.recent_failure_limit,
            },
            response_size_bytes: 0,
        },
        service: service_inventory,
        health,
        telemetry,
        backlogs,
        recent_failures,
        recent_failures_meta,
        causal_hints,
        actions,
    };
    snapshot.snapshot.response_size_bytes = serialized_size(&snapshot);
    if snapshot.snapshot.response_size_bytes > options.max_response_bytes {
        snapshot.snapshot.truncated = true;
    }
    snapshot
}

fn service_diagnostics(service: &Service, options: &DiagnosticsOptions) -> ServiceDiagnostics {
    let (commands, commands_meta) = capped_sorted_strings(
        service.command_names().into_iter().map(str::to_string),
        options.command_limit,
        options.id_limit,
    );
    let (events, events_meta) = capped_sorted_strings(
        service.event_names().into_iter().map(str::to_string),
        options.event_limit,
        options.id_limit,
    );
    let handler_specs = service
        .handler_specs()
        .iter()
        .take(options.handler_limit)
        .map(|spec| handler_spec_diagnostics(spec, options))
        .collect::<Vec<_>>();
    let handler_specs_meta = ListMetadata::new(
        options.handler_limit,
        service.handler_specs().len(),
        handler_specs.len(),
    );
    let plan = service.subscription_plan();
    let (plan_commands, plan_commands_meta) =
        capped_sorted_strings(plan.commands, options.command_limit, options.id_limit);
    let (plan_events, plan_events_meta) =
        capped_sorted_strings(plan.events, options.event_limit, options.id_limit);
    let (transports, transports_meta) = capped_sorted_strings(
        options.transports.clone(),
        options.transport_limit,
        options.id_limit,
    );

    ServiceDiagnostics {
        name: service_label(service.name()),
        version: env!("CARGO_PKG_VERSION").to_string(),
        instance_id: options
            .instance_id
            .as_deref()
            .map(|id| sanitize_id(id, options.id_limit)),
        commands,
        commands_meta,
        events,
        events_meta,
        handler_specs,
        handler_specs_meta,
        subscription_plan: SubscriptionPlanDiagnostics {
            commands: plan_commands,
            commands_meta: plan_commands_meta,
            events: plan_events,
            events_meta: plan_events_meta,
        },
        transports,
        transports_meta,
    }
}

fn handler_spec_diagnostics(
    spec: &HandlerSpec,
    options: &DiagnosticsOptions,
) -> HandlerSpecDiagnostic {
    let (names, names_meta) = capped_sorted_strings(
        spec.names().into_iter().map(str::to_string),
        options.command_limit.max(options.event_limit),
        options.id_limit,
    );
    HandlerSpecDiagnostic {
        names,
        names_meta,
        kind: spec.kind.as_str().to_string(),
        delivery: match spec.delivery {
            DeliveryKind::PointToPoint => "point_to_point",
            DeliveryKind::FanOut => "fan_out",
        }
        .to_string(),
    }
}

async fn backlog_diagnostics(
    options: &DiagnosticsOptions,
) -> (BacklogDiagnostics, Vec<PartialError>) {
    let Some(provider) = &options.backlog_stats else {
        return (
            BacklogDiagnostics {
                outbox: OutboxBacklogDiagnostics {
                    configured: false,
                    pending: None,
                    oldest_pending_age_seconds: None,
                    scan_limit: BACKLOG_STATS_SCAN_LIMIT,
                    truncated: false,
                    error: None,
                },
            },
            Vec::new(),
        );
    };

    match provider().await {
        Ok(stats) => (
            BacklogDiagnostics {
                outbox: OutboxBacklogDiagnostics {
                    configured: true,
                    pending: Some(stats.pending),
                    oldest_pending_age_seconds: stats
                        .oldest_created_at
                        .and_then(|created_at| SystemTime::now().duration_since(created_at).ok())
                        .map(|duration| duration.as_secs_f64()),
                    scan_limit: BACKLOG_STATS_SCAN_LIMIT,
                    truncated: stats.pending >= BACKLOG_STATS_SCAN_LIMIT,
                    error: None,
                },
            },
            Vec::new(),
        ),
        Err(error) => {
            let summary = sanitize_summary(&error.to_string(), options.string_limit);
            (
                BacklogDiagnostics {
                    outbox: OutboxBacklogDiagnostics {
                        configured: true,
                        pending: None,
                        oldest_pending_age_seconds: None,
                        scan_limit: BACKLOG_STATS_SCAN_LIMIT,
                        truncated: false,
                        error: Some(summary.clone()),
                    },
                },
                vec![PartialError {
                    section: "backlogs.outbox".to_string(),
                    error_summary: summary,
                }],
            )
        }
    }
}

fn health_diagnostics(
    service: &ServiceDiagnostics,
    backlogs: &BacklogDiagnostics,
    partial_errors: &[PartialError],
) -> HealthDiagnostics {
    let consumes_bus = !service.subscription_plan.commands.is_empty()
        || !service.subscription_plan.events.is_empty();
    let mut checks = vec![HealthCheck {
        id: "router".to_string(),
        status: "ok".to_string(),
        summary: None,
    }];
    checks.push(match &backlogs.outbox {
        outbox if outbox.error.is_some() => HealthCheck {
            id: "outbox_backlog".to_string(),
            status: "degraded".to_string(),
            summary: outbox.error.clone(),
        },
        outbox if outbox.pending.unwrap_or_default() > 0 => HealthCheck {
            id: "outbox_backlog".to_string(),
            status: "warning".to_string(),
            summary: Some("pending outbox messages present".to_string()),
        },
        outbox if outbox.configured => HealthCheck {
            id: "outbox_backlog".to_string(),
            status: "ok".to_string(),
            summary: None,
        },
        _ => HealthCheck {
            id: "outbox_backlog".to_string(),
            status: "unknown".to_string(),
            summary: Some("no outbox backlog provider configured".to_string()),
        },
    });

    HealthDiagnostics {
        status: if partial_errors.is_empty() {
            "ready".to_string()
        } else {
            "partial".to_string()
        },
        readiness: ReadinessDiagnostics {
            accepts_commands: !service.commands.is_empty(),
            consumes_bus,
            publishes_outbox: backlogs.outbox.configured,
        },
        checks,
    }
}

fn telemetry_diagnostics(options: &DiagnosticsOptions) -> TelemetryDiagnostics {
    let (families, families_meta) = metric_families(options);
    TelemetryDiagnostics {
        metrics: MetricsTelemetry {
            enabled: cfg!(feature = "metrics"),
            path: cfg!(feature = "metrics").then(|| "/metrics".to_string()),
            families,
            families_meta,
        },
        tracing: TracingTelemetry {
            propagation: "w3c_trace_context".to_string(),
            otel_spans: cfg!(feature = "otel"),
            export: if cfg!(feature = "otel") {
                "otlp"
            } else {
                "disabled"
            }
            .to_string(),
        },
        diagnostics: DiagnosticsTelemetry {
            enabled: true,
            path: options.diagnostics_path.clone(),
            visibility: "private".to_string(),
        },
    }
}

fn metric_families(options: &DiagnosticsOptions) -> (Vec<String>, ListMetadata) {
    #[cfg(feature = "metrics")]
    {
        let snapshot = crate::metrics::snapshot();
        let names = snapshot
            .families()
            .iter()
            .map(|family| family.family.name.to_string());
        capped_strings_preserve_order(names, options.metric_family_limit, options.id_limit)
    }
    #[cfg(not(feature = "metrics"))]
    {
        (
            Vec::new(),
            ListMetadata {
                limit: options.metric_family_limit,
                omitted_count: 0,
                truncated: false,
            },
        )
    }
}

fn recent_failure_snapshot(
    service: &str,
    limit: usize,
) -> (Vec<RecentFailure>, RecentFailuresMetadata) {
    recent_failures()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .snapshot_for_service(service, limit)
}

fn causal_hints(recent_failures: &[RecentFailure]) -> CausalHints {
    let mut trace_ids = BTreeSet::new();
    let mut correlation_ids = BTreeSet::new();
    let mut notes = BTreeSet::new();
    for failure in recent_failures {
        if let Some(trace_id) = &failure.trace_id {
            trace_ids.insert(trace_id.clone());
            notes.insert("traceparent present".to_string());
        }
        if let Some(correlation_id) = &failure.correlation_id {
            correlation_ids.insert(correlation_id.clone());
            notes.insert("correlation_id present".to_string());
        }
        if failure.causation_id.is_some() {
            notes.insert("causation_id present".to_string());
        }
    }
    CausalHints {
        last_trace_ids: trace_ids.into_iter().rev().take(8).collect(),
        last_correlation_ids: correlation_ids.into_iter().rev().take(8).collect(),
        notes: notes.into_iter().collect(),
    }
}

fn suggested_actions(
    backlogs: &BacklogDiagnostics,
    recent_failures: &[RecentFailure],
) -> Vec<SuggestedAction> {
    let mut actions = Vec::new();
    if backlogs.outbox.pending.unwrap_or_default() > 0 {
        actions.push(SuggestedAction {
            id: "outbox_backlog_present".to_string(),
            severity: "warning".to_string(),
            evidence: vec!["outbox.pending > 0".to_string()],
            suggested_next_steps: vec![
                "check transport failure counts".to_string(),
                "inspect broker connectivity".to_string(),
            ],
        });
    }
    if recent_failures
        .iter()
        .any(|failure| failure.kind == "transport")
    {
        actions.push(SuggestedAction {
            id: "recent_transport_failures".to_string(),
            severity: "warning".to_string(),
            evidence: vec!["recent_failures.kind contains transport".to_string()],
            suggested_next_steps: vec![
                "check broker connectivity".to_string(),
                "inspect failure action and retry class".to_string(),
            ],
        });
    }
    if recent_failures
        .iter()
        .any(|failure| failure.error_category == "routing")
    {
        actions.push(SuggestedAction {
            id: "unknown_message_recent".to_string(),
            severity: "info".to_string(),
            evidence: vec!["recent_failures.error_category contains routing".to_string()],
            suggested_next_steps: vec![
                "compare producer message name to service inventory".to_string(),
                "check generated subscription plan".to_string(),
            ],
        });
    }
    actions
}

fn with_age(
    mut snapshot: DiagnosticsSnapshot,
    generated_at: SystemTime,
    now: SystemTime,
) -> DiagnosticsSnapshot {
    snapshot.snapshot.age_ms = duration_ms(now.duration_since(generated_at).unwrap_or_default());
    snapshot.snapshot.response_size_bytes = serialized_size(&snapshot);
    snapshot
}

fn capped_sorted_strings<I>(
    values: I,
    limit: usize,
    string_limit: usize,
) -> (Vec<String>, ListMetadata)
where
    I: IntoIterator<Item = String>,
{
    let sorted = values
        .into_iter()
        .map(|value| sanitize_label(&value, string_limit))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let total = sorted.len();
    let items = sorted.into_iter().take(limit).collect::<Vec<_>>();
    let meta = ListMetadata::new(limit, total, items.len());
    (items, meta)
}

#[cfg(feature = "metrics")]
fn capped_strings_preserve_order<I>(
    values: I,
    limit: usize,
    string_limit: usize,
) -> (Vec<String>, ListMetadata)
where
    I: IntoIterator<Item = String>,
{
    let all = values
        .into_iter()
        .map(|value| sanitize_label(&value, string_limit))
        .collect::<Vec<_>>();
    let total = all.len();
    let items = all.into_iter().take(limit).collect::<Vec<_>>();
    let meta = ListMetadata::new(limit, total, items.len());
    (items, meta)
}

fn recent_failures() -> &'static Mutex<RecentFailureRingBuffer> {
    RECENT_FAILURES.get_or_init(|| Mutex::new(RecentFailureRingBuffer::default()))
}

fn handler_error_category(error: &HandlerError) -> &'static str {
    match error {
        HandlerError::UnknownCommand(_) => "routing",
        HandlerError::DecodeFailed(_) => "decode",
        HandlerError::Rejected(_) => "validation",
        HandlerError::NotFound(_) => "repository",
        HandlerError::Unauthorized(_) => "auth",
        HandlerError::Repository(_) => "repository",
        HandlerError::GuardRejected(_) => "guard",
        HandlerError::Other(_) => "handler",
    }
}

fn transport_error_class(kind: TransportErrorKind) -> &'static str {
    match kind {
        TransportErrorKind::Retryable => "retryable",
        TransportErrorKind::Permanent => "permanent",
    }
}

fn runbook_keys_for(category: &str, action: &str, kind: &str) -> Vec<String> {
    let mut keys = Vec::new();
    match category {
        "routing" => keys.extend(["check_service_inventory", "check_message_name"]),
        "decode" => keys.extend(["check_payload_codec", "check_producer_schema"]),
        "auth" => keys.extend(["check_auth_proxy"]),
        "repository" | "storage" => keys.extend(["check_store_connectivity"]),
        "transport" => keys.extend(["check_broker_connectivity"]),
        "outbox_publish" => keys.extend(["check_broker_connectivity", "inspect_outbox_backlog"]),
        _ => keys.extend(["inspect_recent_failure"]),
    }
    if action == "nack" || action == "release" {
        keys.push("watch_retry_backoff");
    }
    if kind == "outbox" {
        keys.push("inspect_outbox_backlog");
    }
    keys.sort_unstable();
    keys.dedup();
    keys.into_iter().map(str::to_string).collect()
}

fn sanitize_id(value: &str, limit: usize) -> String {
    sanitize_label(value, limit)
}

fn sanitize_label(value: &str, limit: usize) -> String {
    if looks_sensitive(value) {
        return "[redacted]".to_string();
    }
    truncate_chars(value, limit)
}

fn sanitize_summary(value: &str, limit: usize) -> String {
    if looks_sensitive(value) {
        return "[redacted]".to_string();
    }
    truncate_chars(value, limit)
}

fn looks_sensitive(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    let sensitive_terms = [
        "authorization",
        "bearer ",
        "cookie",
        "password",
        "passwd",
        "secret",
        "token",
        "api_key",
        "apikey",
        "private_key",
        "database_url",
        "db_url",
        "postgres://",
        "postgresql://",
        "mysql://",
        "redis://",
        "amqp://",
    ];
    sensitive_terms.iter().any(|term| lower.contains(term))
}

fn truncate_chars(value: &str, limit: usize) -> String {
    let mut out = String::new();
    for ch in value.chars().take(limit) {
        if ch.is_control() {
            out.push(' ');
        } else {
            out.push(ch);
        }
    }
    out
}

fn parse_traceparent(value: &str) -> Option<TraceParts> {
    if !is_valid_traceparent(value) {
        return None;
    }
    let mut parts = value.split('-');
    let _version = parts.next()?;
    Some(TraceParts {
        trace_id: parts.next()?.to_string(),
        parent_span_id: parts.next()?.to_string(),
        flags: parts.next()?.to_string(),
    })
}

struct TraceParts {
    trace_id: String,
    parent_span_id: String,
    flags: String,
}

fn service_label(service: Option<&str>) -> String {
    service
        .filter(|value| !value.is_empty())
        .unwrap_or("unnamed")
        .to_string()
}

fn format_timestamp(time: SystemTime) -> String {
    OffsetDateTime::from(time)
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

fn duration_ms(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}

fn serialized_size(snapshot: &DiagnosticsSnapshot) -> usize {
    serde_json::to_vec(snapshot).map_or(0, |json| json.len())
}

#[cfg(test)]
fn reset_for_tests() {
    *recent_failures()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = RecentFailureRingBuffer::default();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::microsvc::{Context, Routes};
    use serde_json::json;
    use std::sync::OnceLock;

    static TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    fn lock_for_tests() -> StdMutexGuard<'static, ()> {
        TEST_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn failure(service: &str, message: &str) -> RecentFailure {
        RecentFailure {
            at: "2026-07-07T00:00:00Z".to_string(),
            service: service.to_string(),
            kind: "transport".to_string(),
            component: "transport".to_string(),
            operation: "receive".to_string(),
            class: "retryable".to_string(),
            action: "nack".to_string(),
            transport: Some("nats".to_string()),
            message_kind: Some("event".to_string()),
            message: message.to_string(),
            error_category: "transport".to_string(),
            error_summary: "timed out".to_string(),
            correlation_id: None,
            causation_id: None,
            trace_id: None,
            parent_span_id: None,
            trace_flags: None,
            runbook_keys: vec!["check_broker_connectivity".to_string()],
        }
    }

    fn service() -> Service {
        Service::new().named("diag-unit").routes(
            Routes::new()
                .with_dependencies(())
                .command("orders.create")
                .handle(|_ctx: &Context<()>| async move { Ok(json!({"ok": true})) }),
        )
    }

    #[test]
    fn recent_failure_ring_buffer_evicts_oldest_and_counts_drops() {
        let mut buffer = RecentFailureRingBuffer::new(2);

        buffer.push(failure("orders", "first"));
        buffer.push(failure("orders", "second"));
        buffer.push(failure("orders", "third"));

        let (records, meta) = buffer.snapshot_for_service("orders", 10);
        assert_eq!(
            records
                .iter()
                .map(|record| record.message.as_str())
                .collect::<Vec<_>>(),
            vec!["second", "third"]
        );
        assert_eq!(meta.capacity, 2);
        assert_eq!(meta.dropped_count, 1);
    }

    #[test]
    fn recent_failure_snapshot_reports_omitted_count_when_limited() {
        let mut buffer = RecentFailureRingBuffer::new(5);
        for message in ["one", "two", "three"] {
            buffer.push(failure("orders", message));
        }

        let (records, meta) = buffer.snapshot_for_service("orders", 2);

        assert_eq!(
            records
                .iter()
                .map(|record| record.message.as_str())
                .collect::<Vec<_>>(),
            vec!["two", "three"]
        );
        assert_eq!(meta.omitted_count, 1);
        assert!(meta.truncated);
    }

    #[test]
    fn sanitizer_redacts_obvious_secret_material() {
        assert_eq!(
            sanitize_summary(
                "DATABASE_URL=postgres://user:secret@example/db token=abc",
                DEFAULT_STRING_LIMIT
            ),
            "[redacted]"
        );
        assert_eq!(
            sanitize_label("orders.created", DEFAULT_ID_LIMIT),
            "orders.created"
        );
    }

    #[tokio::test]
    async fn snapshot_cache_reuses_generated_at_until_ttl_expires() {
        let _guard = lock_for_tests();
        reset_for_tests();
        let service = service();
        let diagnostics =
            Diagnostics::new(DiagnosticsOptions::new().with_ttl(Duration::from_secs(60)));

        let first = diagnostics.snapshot(&service).await;
        tokio::time::sleep(Duration::from_millis(15)).await;
        let second = diagnostics.snapshot(&service).await;

        assert_eq!(first.generated_at, second.generated_at);
        assert!(second.snapshot.age_ms >= first.snapshot.age_ms);

        diagnostics.invalidate();
        let third = diagnostics.snapshot(&service).await;
        assert_ne!(second.snapshot.age_ms, third.snapshot.age_ms);
    }

    #[tokio::test]
    async fn snapshot_marks_partial_outbox_errors_and_bounds_response() {
        let _guard = lock_for_tests();
        reset_for_tests();
        let service = service();
        let diagnostics = Diagnostics::new(
            DiagnosticsOptions::new()
                .with_recent_failure_limit(2)
                .with_max_response_bytes(16 * 1024)
                .with_outbox_backlog_provider(|| async {
                    Err(RepositoryError::Model(
                        "DATABASE_URL=postgres://secret".to_string(),
                    ))
                }),
        );

        let snapshot = diagnostics.snapshot(&service).await;
        let json = serde_json::to_string(&snapshot).unwrap();

        assert!(snapshot.snapshot.partial);
        assert_eq!(
            snapshot.snapshot.partial_errors[0].section,
            "backlogs.outbox"
        );
        assert!(!json.contains("postgres://secret"));
        assert!(json.len() <= snapshot.snapshot.limits.max_response_bytes);
    }

    #[tokio::test]
    async fn command_lists_are_capped_with_omitted_metadata() {
        let service = Service::new().named("diag-capped").routes(
            Routes::new()
                .with_dependencies(())
                .command("orders.a")
                .handle(|_ctx: &Context<()>| async move { Ok(json!({})) })
                .command("orders.b")
                .handle(|_ctx: &Context<()>| async move { Ok(json!({})) }),
        );
        let diagnostics = Diagnostics::new(DiagnosticsOptions::new().with_command_limit(1));

        let snapshot = diagnostics.snapshot(&service).await;

        assert_eq!(snapshot.service.commands.len(), 1);
        assert_eq!(snapshot.service.commands_meta.omitted_count, 1);
        assert!(snapshot.snapshot.truncated);
    }
}
