{{- define "e2e-ui-ui.name" -}}
{{- default .Chart.Name .Values.name | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{- define "e2e-ui-ui.labels" -}}
app.kubernetes.io/name: {{ include "e2e-ui-ui.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- end -}}
