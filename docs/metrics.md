# Metrics

Distributed exposes framework-owned metrics behind the optional `metrics`
feature. The feature has no OpenTelemetry SDK or Prometheus client dependency:
the crate records bounded counters/gauges internally and renders Prometheus text
from the HTTP `/metrics` endpoint when both `http` and `metrics` are enabled.

## Enabling

```toml
distributed = { version = "...", features = ["http", "metrics"] }
```

Then expose a named HTTP service:

```rust
let service = std::sync::Arc::new(
    distributed::microsvc::Service::new()
        .named("orders")
        .routes(routes),
);
distributed::microsvc::serve(service, "0.0.0.0:3000").await?;
```

Prometheus can scrape `GET /metrics` on the service's HTTP port. Hops
ObserveStack should use the same scrape target and labels; dashboards can join
these metrics with trace data by the stable `service` and message labels.

The scrape route is unauthenticated by design. Do not expose `/metrics` on a
public listener; keep it behind a private network, ingress policy, security
group, or equivalent access control used only by the metrics collector.

For services whose primary transport is not HTTP, run a metrics-only listener on
a side port:

```rust
distributed::metrics::serve_http("0.0.0.0:9100", Some("orders-worker")).await?;
```

You can also compose the router into an existing axum app:

```rust
let metrics = distributed::metrics::http_router_for_service("orders-worker");
```

This listener exposes only `GET /metrics`; it does not expose command dispatch.

## Labels

Metric labels are intentionally bounded:

- `service`: `Service::named(...)`, or `unnamed` when no service name was set.
- `message_kind`: `command` or `event`.
- `message`: registered command/event name; unknown-command failures are
  bucketed as `unknown` rather than recording the unrecognized input.
- `status`: bounded dispatch status such as `success`, `unknown_command`,
  `decode_failed`, `guard_rejected`, `repository_error`, or `other_error`.
- `transport`: built-in source label such as `in_memory`, `sqlite`,
  `postgres`, `rabbitmq`, `kafka`, `nats`, or `outbox`.
- `outcome`: bounded settle/publish outcome such as `ack`, `nack`,
  `dead_letter`, `park`, `ignored`, `log_and_ack`, `published`, `released`, or
  `failed`.
- `failure_class`: `retryable` or `permanent`.
- `action`: failure-policy action such as `nack`, `dead_letter`, `park`,
  `log_and_ack`, `stop`, `recv_error`, or a settle failure label such as
  `settle_ack`.
- `source`: bounded outbox claim source: `dispatcher_batch`,
  `dispatcher_ids`, or `transport_source`.
- `phase`: outbox age observation phase: `claimed` or `settled`.
- `attempt_bucket`: retry attempt bucket: `2`, `3`, `4_10`, or `gt10`.

Do not add IDs, trace IDs, user IDs, aggregate IDs, or other high-cardinality
values as framework labels. Framework labels also must not include worker IDs,
destinations, topics, queues, subjects, raw error strings, metadata values, or
payload values.

The label names, framework status values, transport outcomes, failure actions,
and outbox outcomes live in one internal telemetry vocabulary. New framework
metrics should use that vocabulary rather than introducing ad hoc labels.

## Metric Families

- `distributed_service_info{service,version}` gauge.
- `distributed_microsvc_dispatch_total{service,message_kind,message,status}`
  counter.
- `distributed_microsvc_dispatch_duration_seconds{service,message_kind,message,status}`
  histogram.
- `distributed_transport_messages_total{service,transport,message_kind,outcome}`
  counter.
- `distributed_transport_failures_total{service,transport,failure_class,action}`
  counter.
- `distributed_transport_receive_duration_seconds{service,transport,outcome}`
  histogram for `recv()` calls. Outcomes are `message`, `drained`, and `error`.
- `distributed_transport_in_flight_duration_seconds{service,transport,message_kind,outcome}`
  histogram from receive to successful settlement, settle failure, or stop.
- `distributed_transport_message_age_seconds{service,transport,message_kind}`
  histogram when an adapter supplies a reliable producer/store timestamp.
- `distributed_transport_delivery_attempts_total{service,transport,message_kind,attempt_bucket,outcome}`
  counter when an adapter supplies a delivery attempt. First deliveries are not
  recorded; retries start at bucket `2`.
- `distributed_outbox_messages_total{service,outcome}` counter.
- `distributed_outbox_pending_messages{service}` gauge.
- `distributed_outbox_oldest_pending_age_seconds{service}` gauge.
- `distributed_outbox_claim_duration_seconds{service,source,outcome}`
  histogram for claim calls. Outcomes are `success`, `empty`, and `error`.
- `distributed_outbox_claimed_messages_total{service,source}` counter.
- `distributed_outbox_publish_duration_seconds{service,message_kind,outcome}`
  histogram for outbox publish attempts.
- `distributed_outbox_message_age_seconds{service,phase,outcome}` histogram.
- `distributed_outbox_retry_messages_total{service,outcome,attempt_bucket}`
  counter for retry attempts only.
- `distributed_outbox_claimable_messages{service}` gauge.
- `distributed_outbox_in_flight_messages{service}` gauge.
- `distributed_outbox_oldest_in_flight_age_seconds{service}` gauge.
- `distributed_outbox_stale_leases{service}` gauge.

The registry also exposes an internal typed snapshot shape used by tests and
future private diagnostics. The snapshot contains metric families, bounded
labels, and numeric samples only; diagnostics must not add payloads, metadata,
trace ids, aggregate ids, user ids, raw HTTP targets, or request ids to that
shape.

## Boundaries

The `metrics` feature owns Prometheus text exposition for framework metrics. It
does not install an OpenTelemetry metrics SDK, emit request-level HTTP metrics,
or record user payload data.

Direct transport receive paths emit receive/settle counters, receive and
in-flight histograms, optional adapter-supplied age/attempt signals, and failure
counters. Outbox dispatch emits claim timing, publish timing, message age, retry
buckets, publish outcomes, and backlog/runtime gauges. Direct
`MessagePublisher` calls outside the outbox path do not emit publish metrics in
this release; instrumenting those calls should use the same bounded vocabulary
and should stay separate from outbox publish outcomes.

Backlog gauges are cheap store summaries. They do not poll broker admin APIs and
default store implementations stay bounded; SQL stores use aggregate queries.
Alert on sustained trends rather than a single scrape: old pending age can spike
during planned deploy pauses, stale leases imply a worker died or exceeded its
lease, and retry buckets indicate repeated publish/settle attempts rather than a
unique-message count.

## Scaffolded GitOps

`dctl scaffold --gitops --metrics prometheus` adds the Distributed `metrics`
feature to the generated service and emits Prometheus Operator resources for HTTP
deployments:

- `.gitops/deploy/templates/servicemonitor.yaml`
- `.gitops/deploy/templates/prometheusrule.yaml`

Plain `--gitops` does not emit `monitoring.coreos.com` resources, so generated
charts still apply to clusters that do not have Prometheus Operator CRDs
installed. Even when the templates are generated, `serviceMonitor.enabled` and
`prometheusRule.enabled` default to `false`; enable them in the environment's
Helm values only when the cluster has Prometheus Operator installed.

Knative scaffolds enable the runtime metrics feature and expose the same
`GET /metrics` endpoint from the CloudEvents router, but they do not emit a
`ServiceMonitor`; Knative monitoring topology should be modeled by the platform
layer.

The generated `PrometheusRule` contains conservative starting alerts for:

- elevated microsvc dispatch error rate;
- oldest pending outbox message age;
- stale outbox leases and growing in-flight/backlog depth;
- slow outbox claims or publishes;
- slow transport receive/in-flight windows;
- repeated retryable transport failures.
