### What's changed in v3.1.0

* feat: Prometheus metrics endpoint and OTel trace propagation (#100) (by @patrickleet)

  * Add Prometheus metrics endpoint

  * fix: address prometheus metrics review feedback

  Implements [[codex/prometheus-metrics-endpoint]]

  * Add OpenTelemetry trace context propagation (#99)

  * feat: add OpenTelemetry trace context propagation

  Implements [[tasks/opentelemetry-tracing-compatibility]]

  * fix: resolve tracing scaffold review findings

  Implements [[codex/opentelemetry-tracing-compatibility]]

  * fix: preserve local span hierarchy when a parent span is active

  When a dispatch or outbox publish runs inside an already-active local span,
  applying the remote traceparent unconditionally re-parents the framework span
  and breaks the local hierarchy. Only extract the remote parent at trace entry
  (no current span). Found by codex review of #112; lands here because the
  affected code is this PR's.

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01LRjoCyotvAaK3VbhZRd2gq

  * test: docker-level observability integration suites (#112)

  * test: docker-level observability integration suites

  - tests/metrics_exposition: real HTTP scrape of /metrics across all framework
    families, linted with promtool check metrics (PROMTOOL env gate)
  - tests/otel_export: real OTLP pipeline -> OpenTelemetry Collector container;
    asserts distributed.microsvc.dispatch arrives parented to the incoming
    W3C traceparent (endpoint/file env gates)
  - integration-observability.yaml: reusable workflow running both suites plus
    helm template + kubeconform validation of scaffolded ServiceMonitor/
    PrometheusRule/OTLP-env output against published CRD schemas; wired into
    PR-quality and push-main pipelines

  Implements [[tasks/observability-integration-tests]]

  * test: cover nested span parenting in the OTLP export e2e

  The library fix (set_span_parent_from_metadata_if_no_current_span) moved to
  #99 where the affected code lives; this keeps the regression coverage found
  by the codex review.

  ---------

  Co-authored-by: Claude Fable 5 <noreply@anthropic.com>

  * fix: address observability code-review findings

  - knative ingress: tracestate-only HTTP headers no longer delete the
    message's existing traceparent (headers win only with a traceparent)
  - backlog gauges: drop the 5s refresh throttle — refreshes are
    activity-driven with no timer, so any skipped pass froze the gauges
    at stale values after the final drain
  - OutboxStore::backlog_stats default: bounded scan (1000 rows) instead
    of paging the whole outbox; count saturates, oldest stays exact
  - span parenting: one rule everywhere — an active local span wins;
    transport receive now uses the same conditional parenting as
    dispatch/outbox, documented on the helper
  - TraceContext::from_metadata: first match wins on duplicate keys,
    matching Message accessors and OTel span-parent extraction
  - tests: serialize unknown_command against the global metrics registry

  Implements [[tasks/observability-prs-rebase]]

  * fix: restrict scaffolded OTLP protocol

  Implements [[codex/prometheus-metrics-endpoint]]

  * chore: prepare telemetry foundation

  Implements [[tasks/distributed-telemetry-foundation-cleanup-1]]

  ---------

  Co-authored-by: Claude Fable 5 <noreply@anthropic.com>


See full diff: [v3.0.1...v3.1.0](https://github.com/hops-ops/distributed/compare/v3.0.1...v3.1.0)
