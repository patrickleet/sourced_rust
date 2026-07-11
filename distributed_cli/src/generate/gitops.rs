//! GitOps artifact templates: the `.gitops/deploy` Helm chart (HTTP Deployment +
//! Service, or Knative Service + Brokers + Triggers) and the `.gitops/promote`
//! Argo CD / Flux promotion chart. Pure — produces `GeneratedFile`s.

use std::collections::BTreeSet;

use super::names::{
    command_broker_for_message, event_broker_for_message, k8s_name, KnativeTrigger,
};
use super::{file, Scaffold};
use crate::{GeneratedFile, GitopsPromoteTarget, MetricsTarget, ServiceTransport};

impl Scaffold {
    /// The `.gitops/deploy` (and optional `.gitops/promote`) files. The deploy
    /// chart is emitted whenever any GitOps/GitHub option is set, because the
    /// promotion charts target `.gitops/deploy`.
    pub(super) fn gitops_files(&self) -> Vec<GeneratedFile> {
        let mut files = Vec::new();
        let want_deploy = self.gitops
            || self.gitops_promote.is_some()
            || self.github.is_some()
            || self.github_preview.is_some()
            || self.github_promote.is_some();
        if want_deploy {
            files.push(file(
                ".gitops/deploy/Chart.yaml",
                self.gitops_deploy_chart_yaml(),
            ));
            files.push(file(
                ".gitops/deploy/values.yaml",
                self.gitops_deploy_values_yaml(),
            ));
            match self.transport {
                ServiceTransport::Http => {
                    files.push(file(
                        ".gitops/deploy/templates/deployment.yaml",
                        self.gitops_http_deployment_yaml(),
                    ));
                    files.push(file(
                        ".gitops/deploy/templates/service.yaml",
                        self.gitops_http_service_yaml(),
                    ));
                    if self.metrics == Some(MetricsTarget::Prometheus) {
                        files.push(file(
                            ".gitops/deploy/templates/servicemonitor.yaml",
                            self.gitops_service_monitor_yaml(),
                        ));
                        files.push(file(
                            ".gitops/deploy/templates/prometheusrule.yaml",
                            self.gitops_prometheus_rule_yaml(),
                        ));
                    }
                }
                ServiceTransport::Knative => {
                    files.push(file(
                        ".gitops/deploy/templates/knative-service.yaml",
                        self.gitops_knative_service_yaml(),
                    ));
                    files.push(file(
                        ".gitops/deploy/templates/knative-brokers.yaml",
                        self.gitops_knative_brokers_yaml(),
                    ));
                    files.push(file(
                        ".gitops/deploy/templates/knative-triggers.yaml",
                        self.gitops_knative_triggers_yaml(),
                    ));
                }
            }
        }

        if let Some(promote) = self.gitops_promote {
            files.push(file(
                ".gitops/promote/Chart.yaml",
                self.gitops_promote_chart_yaml(promote),
            ));
            files.push(file(
                ".gitops/promote/values.yaml",
                self.gitops_promote_values_yaml(),
            ));
            match promote {
                GitopsPromoteTarget::Argo => files.push(file(
                    ".gitops/promote/templates/application.yaml",
                    self.gitops_argo_application_yaml(),
                )),
                GitopsPromoteTarget::Flux => files.push(file(
                    ".gitops/promote/templates/helmrelease.yaml",
                    self.gitops_flux_helmrelease_yaml(),
                )),
            }
        }

        files
    }

    /// The container image repository: `ghcr.io/<github owner>/<repo>` when a
    /// GitHub repo is configured, else a default under `hops-ops`. Shared with
    /// the GitHub workflow templates.
    pub(super) fn image_repository(&self) -> String {
        self.github
            .as_ref()
            .map(|g| format!("ghcr.io/{}", g.slug().to_ascii_lowercase()))
            .unwrap_or_else(|| format!("ghcr.io/hops-ops/{}", self.names.package_name))
    }

    fn bus_env_yaml(&self) -> String {
        self.bus
            .map(|bus| {
                format!(
                    r#"            - name: HOPS_BUS
              value: {}
"#,
                    bus.kind()
                )
            })
            .unwrap_or_default()
    }

    fn tracing_env_yaml(&self) -> String {
        if !self.tracing {
            return String::new();
        }

        let service_name = k8s_name(&self.names.package_name);
        format!(
            r#"            {{{{ if .Values.observability.tracing.enabled }}}}
            - name: OTEL_SERVICE_NAME
              value: "{service_name}"
            - name: OTEL_EXPORTER_OTLP_PROTOCOL
              value: "grpc"
            {{{{ if .Values.observability.tracing.otlpEndpoint }}}}
            - name: OTEL_EXPORTER_OTLP_ENDPOINT
              value: {{{{ .Values.observability.tracing.otlpEndpoint | quote }}}}
            {{{{ end }}}}
            {{{{ end }}}}
"#,
        )
    }

    /// Helm env for GraphQL query API: injects `DATABASE_URL` from chart values
    /// when `--query-api` was set (build_with_graphql reads this at runtime).
    fn query_api_env_yaml(&self) -> String {
        if !self.query_api {
            return String::new();
        }

        r#"            {{ if .Values.queryApi.enabled }}
            - name: DATABASE_URL
              value: {{ .Values.queryApi.databaseUrl | quote }}
            {{ end }}
"#
        .to_string()
    }

    fn knative_broker_names(&self) -> Vec<String> {
        let mut brokers = BTreeSet::new();
        for model in &self.models {
            brokers.insert(model.command_broker.clone());
            brokers.insert(model.event_broker.clone());
        }
        for command in &self.commands {
            let broker = self
                .command_model(command)
                .map(|model| model.command_broker.clone())
                .unwrap_or_else(|| command_broker_for_message(&command.message_name));
            brokers.insert(broker);
        }
        for event in &self.events {
            brokers.insert(event_broker_for_message(&event.message_name));
        }
        brokers.into_iter().collect()
    }

    fn knative_triggers(&self) -> Vec<KnativeTrigger> {
        // Different message names can normalize to the same `metadata.name`
        // (e.g. `orders.Created` and `orders.created`). Deduplicate so the
        // generated manifest never declares two Triggers with the same name,
        // which would fail `kubectl apply`.
        let mut seen = BTreeSet::new();
        self.commands
            .iter()
            .map(|command| {
                let broker = self
                    .command_model(command)
                    .map(|model| model.command_broker.clone())
                    .unwrap_or_else(|| command_broker_for_message(&command.message_name));
                KnativeTrigger::new(&command.message_name, &broker, "command")
            })
            .chain(self.events.iter().map(|event| {
                let broker = event_broker_for_message(&event.message_name);
                KnativeTrigger::new(&event.message_name, &broker, "event")
            }))
            .map(|mut trigger| {
                let base = trigger.name.clone();
                let mut suffix = 2;
                while !seen.insert(trigger.name.clone()) {
                    trigger.name = format!("{base}-{suffix}");
                    suffix += 1;
                }
                trigger
            })
            .collect()
    }

    fn gitops_deploy_chart_yaml(&self) -> String {
        format!(
            r#"apiVersion: v2
name: {chart_name}
description: Deploy chart for {service_name}
type: application
version: 0.1.0
appVersion: "0.1.0"
"#,
            chart_name = k8s_name(&format!("{}-deploy", self.names.package_name)),
            service_name = self.names.package_name,
        )
    }

    fn gitops_deploy_values_yaml(&self) -> String {
        let bus = self
            .bus
            .map(|bus| format!("bus:\n  kind: {}\n", bus.kind()))
            .unwrap_or_default();
        let metrics = if self.metrics == Some(MetricsTarget::Prometheus)
            && self.transport == ServiceTransport::Http
        {
            r#"metrics:
  enabled: true
  path: /metrics
  portName: http
serviceMonitor:
  enabled: false
  interval: 30s
  scrapeTimeout: 10s
prometheusRule:
  enabled: false
"#
        } else {
            ""
        };
        let tracing_enabled = self.tracing;
        let query_api_values = if self.query_api {
            r#"queryApi:
  enabled: true
  databaseUrl: ""
"#
        } else {
            ""
        };
        format!(
            r#"image:
  repository: {image_repository}
  tag: latest
service:
  port: 3000
observability:
  tracing:
    enabled: {tracing_enabled}
    otlpEndpoint: ""
{bus}{metrics}{query_api_values}"#,
            image_repository = self.image_repository(),
        )
    }

    fn gitops_http_deployment_yaml(&self) -> String {
        let name = k8s_name(&self.names.package_name);
        let bus_env = self.bus_env_yaml();
        let tracing_env = self.tracing_env_yaml();
        let query_api_env = self.query_api_env_yaml();
        format!(
            r#"apiVersion: apps/v1
kind: Deployment
metadata:
  name: {name}
  labels:
    app.kubernetes.io/name: {name}
    app.kubernetes.io/component: service
    hops.ops.com.ai/service: {name}
spec:
  replicas: 1
  selector:
    matchLabels:
      app.kubernetes.io/name: {name}
  template:
    metadata:
      labels:
        app.kubernetes.io/name: {name}
        app.kubernetes.io/component: service
        hops.ops.com.ai/service: {name}
    spec:
      containers:
        - name: {name}
          image: {{{{ .Values.image.repository }}}}:{{{{ .Values.image.tag }}}}
          ports:
            - containerPort: 3000
          env:
            - name: BIND_ADDR
              value: 0.0.0.0:3000
{bus_env}{tracing_env}{query_api_env}"#,
        )
    }

    fn gitops_http_service_yaml(&self) -> String {
        let name = k8s_name(&self.names.package_name);
        format!(
            r#"apiVersion: v1
kind: Service
metadata:
  name: {name}
  labels:
    app.kubernetes.io/name: {name}
    app.kubernetes.io/component: service
    hops.ops.com.ai/service: {name}
spec:
  selector:
    app.kubernetes.io/name: {name}
  ports:
    - name: http
      port: 80
      targetPort: 3000
"#,
        )
    }

    fn gitops_service_monitor_yaml(&self) -> String {
        let name = k8s_name(&self.names.package_name);
        format!(
            r#"{{{{- if .Values.serviceMonitor.enabled }}}}
apiVersion: monitoring.coreos.com/v1
kind: ServiceMonitor
metadata:
  name: {name}
  labels:
    app.kubernetes.io/name: {name}
spec:
  selector:
    matchLabels:
      app.kubernetes.io/name: {name}
  endpoints:
    - port: {{{{ .Values.metrics.portName }}}}
      path: {{{{ .Values.metrics.path }}}}
      interval: {{{{ .Values.serviceMonitor.interval }}}}
      scrapeTimeout: {{{{ .Values.serviceMonitor.scrapeTimeout }}}}
{{{{- end }}}}
"#
        )
    }

    fn gitops_prometheus_rule_yaml(&self) -> String {
        let name = k8s_name(&self.names.package_name);
        format!(
            r#"{{{{- if .Values.prometheusRule.enabled }}}}
apiVersion: monitoring.coreos.com/v1
kind: PrometheusRule
metadata:
  name: {name}
  labels:
    app.kubernetes.io/name: {name}
spec:
  groups:
    - name: {name}.distributed
      rules:
        - alert: DistributedMicrosvcDispatchErrorsHigh
          expr: |
            (
              sum(rate(distributed_microsvc_dispatch_total{{service="{name}",status!="success"}}[5m]))
              /
              clamp_min(sum(rate(distributed_microsvc_dispatch_total{{service="{name}"}}[5m])), 1)
            ) > 0.05
          for: 10m
          labels:
            severity: warning
          annotations:
            summary: Distributed service {name} has elevated dispatch errors
        - alert: DistributedOutboxOldestPendingHigh
          expr: distributed_outbox_oldest_pending_age_seconds{{service="{name}"}} > 300
          for: 10m
          labels:
            severity: warning
          annotations:
            summary: Distributed service {name} has old pending outbox messages
        - alert: DistributedTransportRetrying
          expr: sum(rate(distributed_transport_failures_total{{service="{name}",failure_class="retryable"}}[5m])) > 0
          for: 15m
          labels:
            severity: warning
          annotations:
            summary: Distributed service {name} is retrying transport work
{{{{- end }}}}
"#
        )
    }

    fn gitops_knative_service_yaml(&self) -> String {
        let name = k8s_name(&self.names.package_name);
        let bus_env = self.bus_env_yaml();
        let tracing_env = self.tracing_env_yaml();
        let query_api_env = self.query_api_env_yaml();
        format!(
            r#"apiVersion: serving.knative.dev/v1
kind: Service
metadata:
  name: {name}
  labels:
    app.kubernetes.io/name: {name}
    app.kubernetes.io/component: service
    hops.ops.com.ai/service: {name}
spec:
  template:
    metadata:
      annotations:
        autoscaling.knative.dev/min-scale: "0"
      labels:
        app.kubernetes.io/name: {name}
        app.kubernetes.io/component: service
        hops.ops.com.ai/service: {name}
    spec:
      containers:
        - image: {{{{ .Values.image.repository }}}}:{{{{ .Values.image.tag }}}}
          ports:
            - containerPort: 3000
          env:
            - name: BIND_ADDR
              value: 0.0.0.0:3000
{bus_env}{tracing_env}{query_api_env}"#,
        )
    }

    fn gitops_knative_brokers_yaml(&self) -> String {
        self.knative_broker_names()
            .into_iter()
            .map(|broker| {
                format!(
                    r#"apiVersion: eventing.knative.dev/v1
kind: Broker
metadata:
  name: {broker}
"#,
                )
            })
            .collect::<Vec<_>>()
            .join("---\n")
    }

    fn gitops_knative_triggers_yaml(&self) -> String {
        let service_name = k8s_name(&self.names.package_name);
        self.knative_triggers()
            .into_iter()
            .map(|trigger| {
                format!(
                    r#"apiVersion: eventing.knative.dev/v1
kind: Trigger
metadata:
  name: {name}
spec:
  broker: {broker}
  filter:
    attributes:
      type: {event_type}
  subscriber:
    ref:
      apiVersion: serving.knative.dev/v1
      kind: Service
      name: {service_name}
    uri: /cloudevent/{event_type}
"#,
                    name = trigger.name,
                    broker = trigger.broker,
                    event_type = trigger.event_type,
                )
            })
            .collect::<Vec<_>>()
            .join("---\n")
    }

    fn gitops_promote_chart_yaml(&self, promote: GitopsPromoteTarget) -> String {
        let suffix = match promote {
            GitopsPromoteTarget::Argo => "argo",
            GitopsPromoteTarget::Flux => "flux",
        };
        format!(
            r#"apiVersion: v2
name: {chart_name}
description: GitOps promotion chart for {service_name}
type: application
version: 0.1.0
appVersion: "0.1.0"
"#,
            chart_name = k8s_name(&format!("{}-{}-promote", self.names.package_name, suffix)),
            service_name = self.names.package_name,
        )
    }

    fn gitops_promote_values_yaml(&self) -> String {
        r#"repoUrl: https://example.invalid/repo.git
targetRevision: HEAD
destinationNamespace: default
"#
        .to_string()
    }

    fn gitops_argo_application_yaml(&self) -> String {
        let name = k8s_name(&self.names.package_name);
        format!(
            r#"apiVersion: argoproj.io/v1alpha1
kind: Application
metadata:
  name: {name}
spec:
  project: default
  source:
    repoURL: https://example.invalid/repo.git
    targetRevision: HEAD
    path: .gitops/deploy
  destination:
    server: https://kubernetes.default.svc
    namespace: default
  syncPolicy:
    automated:
      prune: true
      selfHeal: true
"#,
        )
    }

    fn gitops_flux_helmrelease_yaml(&self) -> String {
        let name = k8s_name(&self.names.package_name);
        format!(
            r#"apiVersion: source.toolkit.fluxcd.io/v1
kind: GitRepository
metadata:
  name: {name}-gitops
spec:
  interval: 1m
  url: https://example.invalid/repo.git
  ref:
    branch: main
---
apiVersion: helm.toolkit.fluxcd.io/v2
kind: HelmRelease
metadata:
  name: {name}
spec:
  interval: 5m
  chart:
    spec:
      chart: .gitops/deploy
      sourceRef:
        kind: GitRepository
        name: {name}-gitops
      interval: 1m
  targetNamespace: default
"#,
        )
    }
}
