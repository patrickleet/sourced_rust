# Observability

Distributed's base observability contract is metadata propagation. The default
feature set has no OpenTelemetry SDK dependency, but the framework preserves W3C
Trace Context across its event, outbox, bus, and service boundaries.

## Trace Context Metadata

Distributed uses these canonical lowercase metadata keys:

| Key | Meaning |
| --- | --- |
| `traceparent` | W3C Trace Context trace and parent span identity |
| `tracestate` | W3C vendor trace state |
| `correlation_id` | Application workflow correlation id |
| `causation_id` | Immediate message or event that caused the work |

`Message` lookup is case-insensitive because wire transports vary in header
casing. When Distributed writes trace keys through `TraceContext`, it writes the
canonical lowercase form and replaces older case variants.

```rust
use distributed::{TraceContext, TRACEPARENT};
use distributed::microsvc::{Message, MessageKind};

let trace = TraceContext {
    traceparent: Some("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01".into()),
    tracestate: Some("vendor=value".into()),
};

let message = Message::new("checkout.started", MessageKind::Event, Vec::new())
    .with_trace_context(&trace);

assert_eq!(message.traceparent(), trace.traceparent.as_deref());
assert_eq!(message.metadata(TRACEPARENT), trace.traceparent.as_deref());
```

Handlers can copy the incoming context into aggregate events or outbox messages:

```rust
# use distributed::{Entity, OutboxMessage};
# use distributed::microsvc::{Context, Message, MessageKind};
# fn example(ctx: &Context<()>) -> Result<(), distributed::EventRecordError> {
let trace = ctx.message().trace_context();

let mut entity = Entity::with_id("checkout-1");
entity.set_trace_context(&trace);
entity.digest_empty("checkout.started")?;

let mut outbox = OutboxMessage::create("evt-1", "checkout.started", b"{}".to_vec())?;
outbox.set_trace_context(&trace);
# Ok(())
# }
```

`EventRecord`, `Entity`, `OutboxMessage`, and `Message` all expose trace context
helpers. Replays do not create spans or mutate trace context; they only read the
metadata already stored with events.

## Optional Span Feature

The `otel` feature adds framework-owned `tracing` spans around HTTP request,
dispatch, handler execution, transport receive, and outbox publish boundaries.
When the incoming HTTP request or message carries W3C `traceparent` /
`tracestate`, Distributed extracts that context and sets it as the OpenTelemetry
parent for the framework span:

```toml
[dependencies]
distributed = { version = "0.1", features = ["http", "otel"] }
```

The library feature intentionally does not install a global tracer or exporter.
Hand-written service binaries should configure their own `tracing` subscriber
and OpenTelemetry layer so deployment owners control sampling, resources,
redaction, and export. `dctl scaffold --tracing` emits a default OTLP tracing
setup in the generated `main.rs` for the common case.

Framework span names are intentionally bounded:

- `GET /health`
- `GET /metrics`
- `POST /{command}`
- `POST /`
- `POST /cloudevent/{type}`
- `distributed.microsvc.dispatch`
- `distributed.handler`
- `distributed.transport.receive`
- `distributed.outbox.publish`

HTTP server spans are created only for Distributed-owned routers and are span
kind `SERVER`. They use bounded HTTP attributes:

- `http.request.method`
- `http.route`
- `http.response.status_code`
- `network.protocol.name = "http"`

Distributed intentionally emits `http.route` and omits `url.path` and
`url.query` in this first pass so route telemetry cannot leak raw paths or query
strings. A command HTTP request carrying `traceparent` creates a request span
parented to that incoming context; `distributed.microsvc.dispatch` is then a
child of the HTTP span. If a caller has already opened a local span before
entering Distributed, the existing ambient-span rule still wins and framework
spans nest under that local span.

Message-oriented spans use these framework-owned message attributes:

- `distributed.message.name`
- `distributed.message.kind`
- `messaging.message.id`

Future spans should use the same helper paths so new attributes are reviewed in
one place. Do not add payload fields, user ids, aggregate ids, raw URI paths,
query strings, headers, cookies, session variables, or raw metadata values as
framework span attributes.

Recommended environment variables for OTLP exporters:

```text
OTEL_SERVICE_NAME=checkout-service
OTEL_RESOURCE_ATTRIBUTES=service.namespace=ticketing,deployment.environment.name=prod
OTEL_EXPORTER_OTLP_ENDPOINT=http://alloy-receiver.monitoring.svc:4317
OTEL_EXPORTER_OTLP_PROTOCOL=grpc
OTEL_TRACES_SAMPLER=parentbased_traceidratio
```

For Hops ObserveStack, prefer exporting to the Alloy receiver and letting
ObserveStack route traces to Tempo:

```text
OTEL_EXPORTER_OTLP_ENDPOINT=http://alloy-receiver.monitoring.svc:4317
OTEL_EXPORTER_OTLP_PROTOCOL=grpc
```

HTTP/protobuf exporters can instead use:

```text
OTEL_EXPORTER_OTLP_TRACES_ENDPOINT=http://alloy-receiver.monitoring.svc:4318/v1/traces
OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf
```

## CLI And GitOps

`dctl scaffold --tracing` enables the generated service's `distributed` `otel`
feature and records tracing intent in `ServiceManifest` with
`TracingManifest::otlp()`. It also adds the generated binary dependencies for
`opentelemetry-otlp`, `tracing-opentelemetry`, and `tracing-subscriber`,
initializes an OTLP tracer provider from `OTEL_EXPORTER_OTLP_*` environment
variables, and shuts the provider down when the server exits. When GitOps output
is requested, the deploy chart renders Helm values for OTLP configuration and
conditionally injects:

- `OTEL_SERVICE_NAME`
- `OTEL_EXPORTER_OTLP_PROTOCOL`
- `OTEL_EXPORTER_OTLP_ENDPOINT` when a chart value is set

`dctl scaffold --metrics prometheus` generates a real `/metrics` endpoint in the
service crate and records `MetricsEndpointManifest::prometheus_default()` in the
service manifest. For generated HTTP Deployment + Service charts, GitOps output
also includes a Prometheus Operator `ServiceMonitor` that scrapes the named
`http` port at `/metrics`.

Knative services can still generate the `/metrics` endpoint, but the generic
GitOps renderer does not emit a `ServiceMonitor` for Knative because the correct
scrape target is platform-specific.

## Integration Tests

CI verifies the observability surface at the docker level (see
`integration-observability.yaml`):

- `tests/metrics_exposition` drives real HTTP + outbox traffic, scrapes
  `GET /metrics`, and lints the exposition with `promtool check metrics`
  (skips when `PROMTOOL` is unset).
- `tests/otel_export` builds a real OTLP pipeline the way a service binary
  would, dispatches a message carrying a W3C `traceparent`, sends an HTTP
  command request carrying `traceparent`, and asserts a real OpenTelemetry
  Collector received the request span and `distributed.microsvc.dispatch` with
  the expected parent/child relationships (skips when
  `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` / `OTEL_COLLECTOR_TRACES_FILE` are
  unset).
- The scaffolded `ServiceMonitor` / `PrometheusRule` / OTLP env output is
  rendered with `helm template` and validated against published CRD schemas
  with `kubeconform`.

Whether a Prometheus Operator actually reconciles the `ServiceMonitor` is a
platform concern, verified on a live cluster rather than per PR.

## Feature Boundaries

The default feature set propagates W3C trace context metadata only.

The `metrics` feature records framework metrics and renders Prometheus text. It
does not expose OpenTelemetry metrics.

The `otel` feature creates framework spans and parents them from W3C HTTP
headers or message metadata when available. It does not install subscribers,
exporters, sampling rules, or resource attributes.

Future `logs` or private `diagnostics` features should build on the same
bounded telemetry vocabulary. Logs may carry structured failure context, but
must not log payloads or secrets by default. Diagnostics should read typed
snapshots of framework state rather than parsing public Prometheus text.
