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
  `dead_letter`, `park`, `ignored`, `published`, `released`, or `failed`.
- `failure_class`: `retryable` or `permanent`.
- `action`: failure-policy action such as `nack`, `dead_letter`, `park`,
  `log_and_ack`, `stop`, or `recv_error`.

Do not add IDs, trace IDs, user IDs, aggregate IDs, or other high-cardinality
values as framework labels.

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
- `distributed_outbox_messages_total{service,outcome}` counter.
- `distributed_outbox_pending_messages{service}` gauge.
- `distributed_outbox_oldest_pending_age_seconds{service}` gauge.

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
