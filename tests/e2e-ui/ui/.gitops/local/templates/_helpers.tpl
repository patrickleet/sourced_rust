{{- define "e2e-ui-ui.name" -}}
{{- default .Chart.Name .Values.name | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{- define "e2e-ui-ui.labels" -}}
app.kubernetes.io/name: {{ include "e2e-ui-ui.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- end -}}

{{- define "e2e-ui-ui.releaseNamespace" -}}
{{- coalesce .Values.environment.namespace .Release.Namespace "default" -}}
{{- end -}}

{{- define "e2e-ui-ui.workspace" -}}
{{- if .Values.identity.workspace -}}
{{- .Values.identity.workspace -}}
{{- else -}}
{{- coalesce .Values.environment.name (include "e2e-ui-ui.releaseNamespace" .) -}}
{{- end -}}
{{- end -}}

{{- define "e2e-ui-ui.identityPrefix" -}}
{{- printf "e2e-ui-%s" (include "e2e-ui-ui.workspace" .) | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{- define "e2e-ui-ui.oidcConnectionSecretName" -}}
{{- $prefix := include "e2e-ui-ui.identityPrefix" . -}}
{{- $generation := int (.Values.identity.oidcGeneration | default 0) -}}
{{- if gt $generation 0 -}}
{{- printf "%s-oidc-conn-g%d" $prefix $generation | trunc 63 | trimSuffix "-" -}}
{{- else -}}
{{- printf "%s-oidc-conn" $prefix | trunc 63 | trimSuffix "-" -}}
{{- end -}}
{{- end -}}
