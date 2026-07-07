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
  `postgres`, `rabbitmq`, `kafka`, `nats`, or `knative`.
- `outcome`: bounded settle/publish outcome such as `ack`, `nack`,
  `dead_letter`, `park`, `ignored`, `log_and_ack`, `published`, `released`, or
  `failed`.
- `failure_class`: `retryable` or `permanent`.
- `action`: failure-policy action such as `nack`, `dead_letter`, `park`,
  `log_and_ack`, `stop`, `recv_error`, or a settle failure label such as
  `settle_ack`.

Do not add IDs, trace IDs, user IDs, aggregate IDs, or other high-cardinality
values as framework labels.

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
- `distributed_transport_publish_total{service,transport,message_kind,outcome}`
  counter for direct bus producer calls. `outcome` is `published` or `failed`.
- `distributed_transport_publish_duration_seconds{service,transport,message_kind,outcome}`
  histogram for direct bus producer call duration.
- `distributed_transport_publish_failures_total{service,transport,message_kind,failure_class}`
  counter for failed direct bus producer calls.
- `distributed_outbox_messages_total{service,outcome}` counter.
- `distributed_outbox_pending_messages{service}` gauge.
- `distributed_outbox_oldest_pending_age_seconds{service}` gauge.

The registry also exposes an internal typed snapshot shape used by tests and
future private diagnostics. The snapshot contains metric families, bounded
labels, and numeric samples only; diagnostics must not add payloads, metadata,
trace ids, aggregate ids, user ids, raw HTTP targets, or request ids to that
shape.

## Boundaries

The `metrics` feature owns Prometheus text exposition for framework metrics. It
does not install an OpenTelemetry metrics SDK, emit request-level HTTP metrics,
or record user payload data.

Direct transport receive paths emit receive/settle counters and failure
counters. Direct `Bus::send`, `Bus::publish`, `send_message`, and
`publish_message` calls on built-in buses emit `distributed_transport_publish_*`
metrics. A direct publish success means the adapter's durable publish threshold
resolved `Ok`: in-memory accepted the message, SQL committed the insert,
RabbitMQ confirmed the publish, Kafka acknowledged per `acks`, NATS JetStream
returned a publish ack, or Knative/HTTP returned a successful broker response.
If that threshold returns `Err`, the publish outcome is `failed` and the
failure counter uses the error's `TransportErrorKind` as `retryable` or
`permanent`.

Outbox-derived publishes are intentionally separate. `BusOutboxPublishHook`,
`BusPublisher`, `DynBusPublisher`, and `OutboxDispatcher` record outbox
`published`, `released`, `failed`, and backlog metrics, but they use the buses'
raw outbox publish path so those rows do not increment direct producer metrics.
This avoids double-counting; outbox metrics and the `distributed.outbox.publish`
span remain the authoritative producer signal for outbox rows.

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
- repeated retryable transport failures.
