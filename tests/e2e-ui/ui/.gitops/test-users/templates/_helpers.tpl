{{- /* Identity helpers owned by the explicit test-users chart. */ -}}
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
Prefers identity.workspace; else the resolved Environment name.
*/}}
{{- define "e2e-ui-ui.workspace" -}}
{{- if .Values.identity.workspace -}}
{{- .Values.identity.workspace -}}
{{- else -}}
{{- coalesce .Values.environment.name (include "e2e-ui-ui.releaseNamespace" .) -}}
{{- end -}}
{{- end -}}

{{/* App namespace: Hops injects environment.namespace; Helm also owns Release.Namespace. */}}
{{- define "e2e-ui-ui.releaseNamespace" -}}
{{- coalesce .Values.environment.namespace .Release.Namespace "default" -}}
{{- end -}}

{{/* Worktree-scoped OIDC app name prefix: e2e-ui-<workspace> */}}
{{- define "e2e-ui-ui.identityPrefix" -}}
{{- printf "e2e-ui-%s" (include "e2e-ui-ui.workspace" .) | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{/*
OIDC MR name. Bump identity.oidcGeneration to rotate provider-generated client
credentials. Environment GitOps prune deletes the previous generation by inventory.
*/}}
{{- define "e2e-ui-ui.oidcResourceName" -}}
{{- $prefix := include "e2e-ui-ui.identityPrefix" . -}}
{{- $generation := int (.Values.identity.oidcGeneration | default 0) -}}
{{- if gt $generation 0 -}}
{{- printf "%s-web-g%d" $prefix $generation | trunc 63 | trimSuffix "-" -}}
{{- else -}}
{{- printf "%s-web" $prefix | trunc 63 | trimSuffix "-" -}}
{{- end -}}
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

{{/*
Crossplane connection secret written by the worktree Oidc MR
(writeConnectionSecretToRef). Holds attribute.client_id / attribute.client_secret
— never commit live client credentials to git.
*/}}
{{- define "e2e-ui-ui.oidcConnectionSecretName" -}}
{{- $prefix := include "e2e-ui-ui.identityPrefix" . -}}
{{- $generation := int (.Values.identity.oidcGeneration | default 0) -}}
{{- if gt $generation 0 -}}
{{- printf "%s-oidc-conn-g%d" $prefix $generation | trunc 63 | trimSuffix "-" -}}
{{- else -}}
{{- printf "%s-oidc-conn" $prefix | trunc 63 | trimSuffix "-" -}}
{{- end -}}
{{- end -}}
