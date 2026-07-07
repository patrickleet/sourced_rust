//! OTLP export end-to-end test against a real OpenTelemetry Collector.
//!
//! The `otel` feature deliberately does not install a tracer or exporter — the
//! service binary owns that wiring. This test plays the service binary's role:
//! it builds a real OTLP pipeline (`opentelemetry-otlp` over HTTP/protobuf,
//! batch processor), dispatches a message carrying a W3C `traceparent` through
//! a `Service`, and asserts the collector received the framework's
//! `distributed.microsvc.dispatch` span with:
//!
//! - the trace id from the incoming `traceparent` (context extraction), and
//! - `parentSpanId` equal to the incoming parent span id (span parenting).
//!
//! Skips unless both env vars are set (see `.github/workflows/
//! integration-observability.yaml` for the collector container setup):
//!
//! - `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` — e.g. `http://localhost:4318/v1/traces`
//! - `OTEL_COLLECTOR_TRACES_FILE` — host path of the collector's file-exporter
//!   output (`/out/traces.json` in the container)
#![cfg(all(feature = "http", feature = "otel"))]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use distributed::bus::{
    run_source, MessagePublisher, MessageSource, ReceivedMessage, RunOptions, TransportError,
};
use distributed::microsvc::{Context, Message, MessageKind, Routes, Service};
use distributed::outbox_worker::OutboxDispatcher;
use distributed::{CommitBatch, InMemoryRepository, OutboxMessage, TransactionalCommit};
use opentelemetry::propagation::{Extractor, TextMapPropagator as _};
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::WithExportConfig as _;
use serde_json::{json, Value};
use tracing::field::{Field, Visit};
use tracing::Instrument as _;
use tracing_opentelemetry::OpenTelemetrySpanExt as _;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::layer::{Context as LayerContext, Layer};
use tracing_subscriber::registry::LookupSpan;

#[path = "../support/env.rs"]
mod env_support;

/// The parent span id we inject; the exported framework span must be its child.
const REMOTE_PARENT_SPAN_ID: &str = "00f067aa0ba902b7";

#[tokio::test(flavor = "current_thread")]
async fn outbox_and_transport_depth_spans_are_recorded_without_exporter_setup() {
    let capture = SpanCaptureLayer::default();
    let subscriber = tracing_subscriber::registry().with(capture.clone());
    let _guard = tracing::subscriber::set_default(subscriber);

    let repo = InMemoryRepository::new();
    let mut outbox =
        OutboxMessage::create("span-outbox-1", "orders.created", b"{}".to_vec()).unwrap();
    outbox.created_at = SystemTime::now() - Duration::from_secs(5);
    repo.commit_batch(CommitBatch {
        outbox_messages: vec![outbox],
        ..CommitBatch::empty()
    })
    .await
    .unwrap();
    let dispatcher = OutboxDispatcher::new(
        repo.outbox_store(),
        SpanPublisher,
        "span-worker",
        Duration::from_secs(60),
        3,
    )
    .with_service("otel-depth");
    dispatcher.dispatch_batch(1).await.unwrap();

    let service = Arc::new(
        Service::new().named("otel-depth").routes(
            Routes::new()
                .with_dependencies(())
                .event("orders.transport")
                .handle(|_ctx: &Context<()>| async move { Ok(json!({"ok": true})) }),
        ),
    );
    run_source(
        service,
        SpanSource {
            next: Some(
                Message::new("orders.transport", MessageKind::Event, b"{}".to_vec())
                    .with_id("transport-1"),
            ),
        },
        RunOptions::idempotent(),
    )
    .await
    .unwrap();

    let spans = capture.snapshot();
    let claim = span_named(&spans, "distributed.outbox.claim");
    assert_field_eq(claim, "distributed.outbox.claim.source", "dispatcher_batch");
    assert_field_eq(claim, "distributed.outbox.claim.requested", "1");
    assert_field_eq(claim, "distributed.outbox.claim.claimed", "1");
    assert_field_eq(claim, "distributed.outbox.outcome", "success");

    let publish = span_named(&spans, "distributed.outbox.publish");
    assert_field_eq(publish, "distributed.outbox.publish.attempt", "1");
    assert_field_contains(publish, "distributed.outbox.message_age_seconds", "5");
    assert_field_eq(publish, "distributed.outbox.outcome", "published");

    let transport = span_named(&spans, "distributed.transport.receive");
    assert_field_eq(transport, "distributed.transport.name", "span-test");
    assert_field_contains(transport, "distributed.transport.delivery_attempt", "4");
    assert_field_contains(transport, "distributed.transport.message_age_seconds", "7");
    assert_field_contains(transport, "distributed.transport.lag", "17");
    assert_field_eq(transport, "distributed.transport.outcome", "success");
}

#[tokio::test]
async fn dispatch_span_reaches_collector_with_propagated_parent() {
    let Some(endpoint) = env_support::broker_env(
        "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
        "otel collector export test",
    ) else {
        return;
    };
    let Some(traces_file) =
        env_support::broker_env("OTEL_COLLECTOR_TRACES_FILE", "otel collector export test")
    else {
        return;
    };

    // A fresh trace id per run so stale collector output can never satisfy the
    // assertion.
    let trace_id = fresh_trace_id(1);
    let traceparent = format!("00-{trace_id}-{REMOTE_PARENT_SPAN_ID}-01");

    // The exporter pipeline a service binary would install.
    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_endpoint(endpoint)
        .build()
        .expect("build OTLP span exporter");
    let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .build();
    let tracer = provider.tracer("otel-export-test");
    let subscriber =
        tracing_subscriber::registry().with(tracing_opentelemetry::layer().with_tracer(tracer));
    let _guard = tracing::subscriber::set_default(subscriber);

    let service = Arc::new(
        Service::new().named("otel-export").routes(
            Routes::new()
                .with_dependencies(())
                .command("orders.create")
                .handle(|_ctx: &Context<()>| async move { Ok(json!({"ok": true})) }),
        ),
    );
    let message = Message::new(
        "orders.create",
        MessageKind::Command,
        br#"{"id":"o-1"}"#.to_vec(),
    )
    .with_id("evt-otel-1")
    .with_metadata("traceparent", &traceparent);

    service
        .dispatch_message(&message)
        .await
        .expect("dispatch succeeds");

    let nested_trace_id = fresh_trace_id(17);
    let nested_traceparent = format!("00-{nested_trace_id}-{REMOTE_PARENT_SPAN_ID}-01");
    let outer_span = tracing::info_span!("test.outer");
    let parent_context = opentelemetry_sdk::propagation::TraceContextPropagator::new().extract(
        &TraceparentExtractor {
            traceparent: &nested_traceparent,
        },
    );
    let _ = outer_span.set_parent(parent_context);
    let nested_message = Message::new(
        "orders.create",
        MessageKind::Command,
        br#"{"id":"o-2"}"#.to_vec(),
    )
    .with_id("evt-otel-nested")
    .with_metadata("traceparent", &nested_traceparent);
    service
        .dispatch_message(&nested_message)
        .instrument(outer_span)
        .await
        .expect("nested dispatch succeeds");

    provider.force_flush().expect("flush spans to collector");
    provider.shutdown().expect("shutdown provider");

    let span = poll_for_span(
        &traces_file,
        &trace_id,
        "distributed.microsvc.dispatch",
        Duration::from_secs(20),
    );
    assert_eq!(
        span["parentSpanId"].as_str(),
        Some(REMOTE_PARENT_SPAN_ID),
        "dispatch span must be parented to the incoming traceparent: {span}"
    );

    let outer_span = poll_for_span(
        &traces_file,
        &nested_trace_id,
        "test.outer",
        Duration::from_secs(20),
    );
    let nested_dispatch_span = poll_for_span(
        &traces_file,
        &nested_trace_id,
        "distributed.microsvc.dispatch",
        Duration::from_secs(20),
    );
    assert_eq!(
        outer_span["parentSpanId"].as_str(),
        Some(REMOTE_PARENT_SPAN_ID),
        "outer span must keep the incoming traceparent as its parent: {outer_span}"
    );
    assert_eq!(
        nested_dispatch_span["parentSpanId"].as_str(),
        outer_span["spanId"].as_str(),
        "dispatch span must preserve the active local span hierarchy: {nested_dispatch_span}"
    );
}

/// Poll the collector's file-exporter output until the framework dispatch span
/// for `trace_id` appears, and return it.
fn poll_for_span(path: &str, trace_id: &str, span_name: &str, timeout: Duration) -> Value {
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(contents) = std::fs::read_to_string(path) {
            for line in contents.lines() {
                let Ok(batch) = serde_json::from_str::<Value>(line) else {
                    continue;
                };
                if let Some(span) = find_span(&batch, trace_id, span_name) {
                    return span;
                }
            }
        }
        assert!(
            Instant::now() < deadline,
            "collector never exported span {span_name} for trace {trace_id}; \
             file {path} contents:\n{}",
            std::fs::read_to_string(path).unwrap_or_else(|e| format!("<unreadable: {e}>"))
        );
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn find_span(batch: &Value, trace_id: &str, span_name: &str) -> Option<Value> {
    for resource_spans in batch["resourceSpans"].as_array()? {
        let Some(scope_spans) = resource_spans["scopeSpans"].as_array() else {
            continue;
        };
        for scope_span in scope_spans {
            let Some(spans) = scope_span["spans"].as_array() else {
                continue;
            };
            for span in spans {
                if span["traceId"].as_str() == Some(trace_id)
                    && span["name"].as_str() == Some(span_name)
                {
                    return Some(span.clone());
                }
            }
        }
    }
    None
}

fn fresh_trace_id(salt: u128) -> String {
    format!(
        "{:032x}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
            .wrapping_add(salt)
            | 1
    )
}

struct TraceparentExtractor<'a> {
    traceparent: &'a str,
}

impl Extractor for TraceparentExtractor<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        key.eq_ignore_ascii_case("traceparent")
            .then_some(self.traceparent)
    }

    fn keys(&self) -> Vec<&str> {
        vec!["traceparent"]
    }
}

struct SpanPublisher;

impl MessagePublisher for SpanPublisher {
    async fn publish(&self, _message: Message) -> Result<(), TransportError> {
        Ok(())
    }
}

struct SpanSource {
    next: Option<Message>,
}

impl MessageSource for SpanSource {
    type Received = SpanReceived;

    fn transport_name(&self) -> &'static str {
        "span-test"
    }

    async fn recv(&mut self) -> Result<Option<Self::Received>, TransportError> {
        Ok(self.next.take().map(|message| SpanReceived { message }))
    }
}

struct SpanReceived {
    message: Message,
}

impl ReceivedMessage for SpanReceived {
    fn message(&self) -> &Message {
        &self.message
    }

    fn delivery_attempt(&self) -> Option<u32> {
        Some(4)
    }

    fn producer_timestamp(&self) -> Option<SystemTime> {
        Some(SystemTime::now() - Duration::from_secs(7))
    }

    fn transport_lag(&self) -> Option<i64> {
        Some(17)
    }

    async fn ack(self) -> Result<(), TransportError> {
        Ok(())
    }

    async fn nack(self, _reason: &str) -> Result<(), TransportError> {
        Ok(())
    }
}

#[derive(Clone, Default)]
struct SpanCaptureLayer {
    spans: Arc<Mutex<BTreeMap<u64, CapturedSpan>>>,
}

impl SpanCaptureLayer {
    fn snapshot(&self) -> Vec<CapturedSpan> {
        self.spans.lock().unwrap().values().cloned().collect()
    }
}

#[derive(Clone, Debug, Default)]
struct CapturedSpan {
    name: String,
    fields: BTreeMap<String, String>,
}

impl<S> Layer<S> for SpanCaptureLayer
where
    S: tracing::Subscriber + for<'lookup> LookupSpan<'lookup>,
{
    fn on_new_span(
        &self,
        attrs: &tracing::span::Attributes<'_>,
        id: &tracing::span::Id,
        _ctx: LayerContext<'_, S>,
    ) {
        let mut fields = BTreeMap::new();
        attrs.record(&mut FieldRecorder {
            fields: &mut fields,
        });
        self.spans.lock().unwrap().insert(
            id.clone().into_u64(),
            CapturedSpan {
                name: attrs.metadata().name().to_string(),
                fields,
            },
        );
    }

    fn on_record(
        &self,
        id: &tracing::span::Id,
        values: &tracing::span::Record<'_>,
        _ctx: LayerContext<'_, S>,
    ) {
        let mut fields = BTreeMap::new();
        values.record(&mut FieldRecorder {
            fields: &mut fields,
        });
        if let Some(span) = self.spans.lock().unwrap().get_mut(&id.clone().into_u64()) {
            span.fields.extend(fields);
        }
    }
}

struct FieldRecorder<'a> {
    fields: &'a mut BTreeMap<String, String>,
}

impl Visit for FieldRecorder<'_> {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.fields
            .insert(field.name().to_string(), format!("{value:?}"));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }
}

fn span_named<'a>(spans: &'a [CapturedSpan], name: &str) -> &'a CapturedSpan {
    spans
        .iter()
        .find(|span| span.name == name)
        .unwrap_or_else(|| panic!("missing span {name}; captured spans: {spans:#?}"))
}

fn assert_field_eq(span: &CapturedSpan, key: &str, expected: &str) {
    assert_eq!(
        span.fields.get(key).map(String::as_str),
        Some(expected),
        "span `{}` should carry `{key}` = `{expected}`; fields: {:#?}",
        span.name,
        span.fields
    );
}

fn assert_field_contains(span: &CapturedSpan, key: &str, expected: &str) {
    let value = span.fields.get(key).unwrap_or_else(|| {
        panic!(
            "span `{}` missing `{key}`; fields: {:#?}",
            span.name, span.fields
        )
    });
    assert!(
        value.contains(expected),
        "span `{}` field `{key}` = `{value}` should contain `{expected}`",
        span.name
    );
}
