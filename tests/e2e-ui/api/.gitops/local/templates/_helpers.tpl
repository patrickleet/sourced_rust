{{- define "e2e-ui-api.name" -}}
{{- default .Chart.Name .Values.name | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{- define "e2e-ui-api.labels" -}}
app.kubernetes.io/name: {{ include "e2e-ui-api.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- end -}}

{{- define "e2e-ui-api.workspace" -}}
{{- if .Values.identity.workspace -}}
{{- .Values.identity.workspace -}}
{{- else -}}
{{- coalesce .Values.environment.name .Values.environment.namespace .Release.Namespace "default" -}}
{{- end -}}
{{- end -}}

{{- define "e2e-ui-api.oidcConnectionSecretName" -}}
{{- if .Values.identity.connectionSecretName -}}
{{- .Values.identity.connectionSecretName | trunc 63 | trimSuffix "-" -}}
{{- else -}}
{{- $prefix := printf "e2e-ui-%s" (include "e2e-ui-api.workspace" .) -}}
{{- $generation := int (.Values.identity.oidcGeneration | default 0) -}}
{{- if gt $generation 0 -}}
{{- printf "%s-oidc-conn-g%d" $prefix $generation | trunc 63 | trimSuffix "-" -}}
{{- else -}}
{{- printf "%s-oidc-conn" $prefix | trunc 63 | trimSuffix "-" -}}
{{- end -}}
{{- end -}}
{{- end -}}

{{- define "e2e-ui-api.oidcClientEnv" -}}
{{- if .Values.identity.enabled }}
{{- $secret := include "e2e-ui-api.oidcConnectionSecretName" . }}
- name: OIDC_AUDIENCE
  valueFrom:
    secretKeyRef:
      name: {{ $secret | quote }}
      key: attribute.client_id
- name: OIDC_CLIENT_ID
  valueFrom:
    secretKeyRef:
      name: {{ $secret | quote }}
      key: attribute.client_id
{{- end }}
{{- end -}}
