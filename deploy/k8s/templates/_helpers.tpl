{{/*
Expand the name of the chart.
*/}}
{{- define "darshjdb.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Create a default fully qualified app name.
*/}}
{{- define "darshjdb.fullname" -}}
{{- if .Values.fullnameOverride }}
{{- .Values.fullnameOverride | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- $name := default .Chart.Name .Values.nameOverride }}
{{- if contains $name .Release.Name }}
{{- .Release.Name | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- printf "%s-%s" .Release.Name $name | trunc 63 | trimSuffix "-" }}
{{- end }}
{{- end }}
{{- end }}

{{/*
Common labels
*/}}
{{- define "darshjdb.labels" -}}
helm.sh/chart: {{ include "darshjdb.name" . }}-{{ .Chart.Version | replace "+" "_" }}
{{ include "darshjdb.selectorLabels" . }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- end }}

{{/*
PostgreSQL connection string: the bundled postgresql subchart when enabled,
otherwise the operator-supplied external database.
*/}}
{{- define "darshjdb.databaseUrl" -}}
{{- if .Values.postgresql.enabled -}}
{{- printf "postgres://%s:%s@%s-postgresql:5432/%s" .Values.postgresql.auth.username .Values.postgresql.auth.password .Release.Name .Values.postgresql.auth.database -}}
{{- else -}}
{{- required "externalDatabase.url is required when postgresql.enabled is false" .Values.externalDatabase.url -}}
{{- end -}}
{{- end }}

{{/*
Selector labels
*/}}
{{- define "darshjdb.selectorLabels" -}}
app.kubernetes.io/name: {{ include "darshjdb.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end }}
