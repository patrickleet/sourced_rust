{{- define "e2e-ui-ui.name" -}}
{{- default .Chart.Name .Values.name | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{- define "e2e-ui-ui.labels" -}}
app.kubernetes.io/name: {{ include "e2e-ui-ui.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- end -}}

{{/*
Workspace id for multi-worktree OIDC apps (redirects / Login V2 baseUri).
Prefers identity.workspace; else release/workspace namespace (= hops --name).
*/}}
{{- define "e2e-ui-ui.workspace" -}}
{{- if .Values.identity.workspace -}}
{{- .Values.identity.workspace -}}
{{- else -}}
{{- include "e2e-ui-ui.releaseNamespace" . -}}
{{- end -}}
{{- end -}}

{{/* App namespace: hops injects values.namespace; helm --namespace is Release.Namespace. */}}
{{- define "e2e-ui-ui.releaseNamespace" -}}
{{- coalesce .Values.namespace .Release.Namespace "default" -}}
{{- end -}}

{{/* Worktree-scoped OIDC app name prefix: e2e-ui-<workspace> */}}
{{- define "e2e-ui-ui.identityPrefix" -}}
{{- printf "e2e-ui-%s" (include "e2e-ui-ui.workspace" .) | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{/* Cluster-shared Zitadel Project MR name (one per control plane). */}}
{{- define "e2e-ui-ui.clusterProjectName" -}}
{{- default "e2e-ui" .Values.identity.projectName -}}
{{- end -}}

{{- define "e2e-ui-ui.identityLabels" -}}
app.kubernetes.io/name: e2e-ui
app.kubernetes.io/component: identity
hops.ops.com.ai/app: e2e-ui
hops.ops.com.ai/workspace: {{ include "e2e-ui-ui.workspace" . | quote }}
{{- end -}}

{{- define "e2e-ui-ui.identityClusterLabels" -}}
app.kubernetes.io/name: e2e-ui
app.kubernetes.io/component: identity
hops.ops.com.ai/app: e2e-ui
hops.ops.com.ai/identity-scope: cluster
{{- end -}}

{{/*
UI public base for OIDC redirects + app Login V2 baseUri.
Uses the release/workspace namespace so --name dogfood → e2e-ui-ui.dogfood.svc…
*/}}
{{- define "e2e-ui-ui.uiBaseURL" -}}
{{- if .Values.identity.uiBaseURL -}}
{{- .Values.identity.uiBaseURL | trimSuffix "/" -}}
{{- else -}}
{{- $ns := include "e2e-ui-ui.releaseNamespace" . -}}
{{- $svc := default (include "e2e-ui-ui.name" .) .Values.identity.uiService -}}
{{- $port := default .Values.service.port .Values.identity.uiPort -}}
{{- printf "http://%s.%s.svc.cluster.local:%v" $svc $ns $port -}}
{{- end -}}
{{- end -}}
