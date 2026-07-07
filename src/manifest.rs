use serde::{Deserialize, Serialize};

use crate::table::{
    generate_table_migration_artifacts, table_schema_statements, TableSchema, TableSchemaRegistry,
    TableSqlDialect,
};
use crate::{RelationalReadModel, TableMigrationArtifact, TableStoreError};

pub const DISTRIBUTED_MANIFEST_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DistributedManifestEnvelope {
    pub schema_version: u32,
    pub project: DistributedProjectManifest,
}

impl DistributedManifestEnvelope {
    pub fn new(project: DistributedProjectManifest) -> Self {
        Self {
            schema_version: DISTRIBUTED_MANIFEST_SCHEMA_VERSION,
            project,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DistributedProjectManifest {
    pub name: String,
    pub tables: Vec<TableSchema>,
    pub services: Vec<ServiceManifest>,
}

impl DistributedProjectManifest {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            tables: Vec::new(),
            services: Vec::new(),
        }
    }

    pub fn read_model<M>(mut self) -> Self
    where
        M: RelationalReadModel,
    {
        self.try_register_read_model::<M>()
            .expect("read model schema should be valid in distributed manifest");
        self
    }

    pub fn try_read_model<M>(mut self) -> Result<Self, TableStoreError>
    where
        M: RelationalReadModel,
    {
        self.try_register_read_model::<M>()?;
        Ok(self)
    }

    pub fn try_register_read_model<M>(&mut self) -> Result<&mut Self, TableStoreError>
    where
        M: RelationalReadModel,
    {
        self.try_register_table_schema(M::schema().clone())
    }

    pub fn table_schema(mut self, schema: TableSchema) -> Self {
        self.try_register_table_schema(schema)
            .expect("table schema should be valid in distributed manifest");
        self
    }

    pub fn try_table_schema(mut self, schema: TableSchema) -> Result<Self, TableStoreError> {
        self.try_register_table_schema(schema)?;
        Ok(self)
    }

    pub fn try_register_table_schema(
        &mut self,
        schema: TableSchema,
    ) -> Result<&mut Self, TableStoreError> {
        let mut registry = self.table_registry()?;
        registry.register_schema(schema.clone())?;
        self.tables.push(schema);
        Ok(self)
    }

    pub fn service(mut self, service: ServiceManifest) -> Self {
        self.services.push(service);
        self
    }

    pub fn table_registry(&self) -> Result<TableSchemaRegistry, TableStoreError> {
        let mut registry = TableSchemaRegistry::new();
        for schema in &self.tables {
            registry.register_schema(schema.clone())?;
        }
        Ok(registry)
    }

    pub fn sql_statements(&self, dialect: TableSqlDialect) -> Result<Vec<String>, TableStoreError> {
        table_schema_statements(&self.table_registry()?, dialect)
    }

    pub fn sql_migration_artifacts(
        &self,
        dialect: TableSqlDialect,
    ) -> Result<Vec<TableMigrationArtifact>, TableStoreError> {
        generate_table_migration_artifacts(&self.table_registry()?, dialect)
    }

    pub fn envelope(self) -> DistributedManifestEnvelope {
        DistributedManifestEnvelope::new(self)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceManifest {
    pub name: String,
    pub commands: Vec<MessageEndpointManifest>,
    pub events: Vec<MessageEndpointManifest>,
    pub transports: Vec<TransportManifest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observability: Option<ServiceObservabilityManifest>,
}

impl ServiceManifest {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            commands: Vec::new(),
            events: Vec::new(),
            transports: Vec::new(),
            observability: None,
        }
    }

    pub fn command(mut self, name: impl Into<String>) -> Self {
        self.commands.push(MessageEndpointManifest::new(name));
        self
    }

    pub fn event(mut self, name: impl Into<String>) -> Self {
        self.events.push(MessageEndpointManifest::new(name));
        self
    }

    pub fn transport(mut self, kind: impl Into<String>) -> Self {
        self.transports.push(TransportManifest::new(kind));
        self
    }

    pub fn observability(mut self, observability: ServiceObservabilityManifest) -> Self {
        self.observability = Some(observability);
        self
    }

    pub fn metrics(mut self, metrics: MetricsEndpointManifest) -> Self {
        let mut observability = self.observability.unwrap_or_default();
        observability.metrics = Some(metrics);
        self.observability = Some(observability);
        self
    }

    pub fn tracing(mut self, tracing: TracingManifest) -> Self {
        let mut observability = self.observability.unwrap_or_default();
        observability.tracing = Some(tracing);
        self.observability = Some(observability);
        self
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceObservabilityManifest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metrics: Option<MetricsEndpointManifest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracing: Option<TracingManifest>,
}

impl ServiceObservabilityManifest {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn metrics(mut self, metrics: MetricsEndpointManifest) -> Self {
        self.metrics = Some(metrics);
        self
    }

    pub fn tracing(mut self, tracing: TracingManifest) -> Self {
        self.tracing = Some(tracing);
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetricsEndpointManifest {
    pub path: String,
    pub port_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval: Option<String>,
}

impl MetricsEndpointManifest {
    pub fn new(path: impl Into<String>, port_name: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            port_name: port_name.into(),
            interval: None,
        }
    }

    pub fn prometheus_default() -> Self {
        Self::new("/metrics", "http").interval("30s")
    }

    pub fn interval(mut self, interval: impl Into<String>) -> Self {
        self.interval = Some(interval.into());
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TracingManifest {
    pub propagation: TracePropagationMode,
    pub export: TraceExportMode,
}

impl TracingManifest {
    pub fn otlp() -> Self {
        Self {
            propagation: TracePropagationMode::W3cTraceContext,
            export: TraceExportMode::Otlp,
        }
    }

    pub fn disabled() -> Self {
        Self {
            propagation: TracePropagationMode::Disabled,
            export: TraceExportMode::Disabled,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TracePropagationMode {
    #[default]
    W3cTraceContext,
    Disabled,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceExportMode {
    #[default]
    Otlp,
    Disabled,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageEndpointManifest {
    pub name: String,
}

impl MessageEndpointManifest {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransportManifest {
    pub kind: String,
}

impl TransportManifest {
    pub fn new(kind: impl Into<String>) -> Self {
        Self { kind: kind.into() }
    }

    pub fn http() -> Self {
        Self::new("http")
    }
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};

    use super::*;
    use crate::{outbox_message_schema, ReadModel};

    #[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ReadModel)]
    #[table("orders")]
    struct OrderView {
        #[id("order_id")]
        order_id: String,
        status: String,
    }

    #[test]
    fn manifest_collects_schema_service_metadata_and_renders_sql() {
        let manifest = DistributedProjectManifest::new("checkout")
            .read_model::<OrderView>()
            .table_schema(outbox_message_schema().clone())
            .service(
                ServiceManifest::new("checkout-saga")
                    .command("checkout.start")
                    .event("seat.reserved")
                    .transport("http"),
            );

        let envelope = DistributedManifestEnvelope::new(manifest.clone());
        let json = serde_json::to_string(&envelope).expect("manifest should serialize");
        assert!(json.contains("\"schema_version\":1"));
        assert!(json.contains("\"table_name\":\"orders\""));

        let restored: DistributedManifestEnvelope =
            serde_json::from_str(&json).expect("manifest should deserialize");
        assert_eq!(restored.project.name, "checkout");
        assert_eq!(restored.project.tables.len(), 2);
        assert_eq!(
            restored.project.services[0].commands[0].name,
            "checkout.start"
        );

        let sql = manifest
            .sql_statements(TableSqlDialect::Postgres)
            .expect("manifest SQL should render")
            .join("\n");
        assert!(sql.contains("CREATE TABLE IF NOT EXISTS \"orders\""));
        assert!(sql.contains("CREATE TABLE IF NOT EXISTS \"outbox_messages\""));
    }

    #[test]
    fn service_manifest_serializes_observability_metadata_when_declared() {
        let service = ServiceManifest::new("checkout-saga")
            .metrics(MetricsEndpointManifest::prometheus_default())
            .tracing(TracingManifest::otlp());

        let json = serde_json::to_string(&service).expect("service manifest should serialize");
        assert!(json.contains("\"observability\""));
        assert!(json.contains("\"path\":\"/metrics\""));
        assert!(json.contains("\"propagation\":\"w3c_trace_context\""));

        let restored: ServiceManifest =
            serde_json::from_str(&json).expect("service manifest should deserialize");
        let observability = restored
            .observability
            .expect("observability should deserialize");
        assert_eq!(
            observability.metrics.expect("metrics").port_name,
            "http".to_string()
        );
        assert_eq!(
            observability.tracing.expect("tracing").export,
            TraceExportMode::Otlp
        );
    }

    #[test]
    fn service_manifest_observability_is_optional_for_older_json() {
        let json = r#"{"name":"checkout-saga","commands":[],"events":[],"transports":[]}"#;
        let restored: ServiceManifest =
            serde_json::from_str(json).expect("older service manifest should deserialize");

        assert!(restored.observability.is_none());
    }
}
