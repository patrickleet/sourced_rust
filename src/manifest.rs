use serde::{Deserialize, Serialize};

use crate::table::{
    generate_table_migration_artifacts, table_schema_statements, TableSchema, TableSchemaRegistry,
    TableSqlDialect,
};
use crate::{ReadModelError, ReadModelMigrationArtifact, RelationalReadModel};

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

    pub fn try_read_model<M>(mut self) -> Result<Self, ReadModelError>
    where
        M: RelationalReadModel,
    {
        self.try_register_read_model::<M>()?;
        Ok(self)
    }

    pub fn try_register_read_model<M>(&mut self) -> Result<&mut Self, ReadModelError>
    where
        M: RelationalReadModel,
    {
        self.try_register_table_schema(M::schema())
    }

    pub fn table_schema(mut self, schema: TableSchema) -> Self {
        self.try_register_table_schema(schema)
            .expect("table schema should be valid in distributed manifest");
        self
    }

    pub fn try_table_schema(mut self, schema: TableSchema) -> Result<Self, ReadModelError> {
        self.try_register_table_schema(schema)?;
        Ok(self)
    }

    pub fn try_register_table_schema(
        &mut self,
        schema: TableSchema,
    ) -> Result<&mut Self, ReadModelError> {
        let mut registry = self.table_registry()?;
        registry.register_schema(schema.clone())?;
        self.tables.push(schema);
        Ok(self)
    }

    pub fn service(mut self, service: ServiceManifest) -> Self {
        self.services.push(service);
        self
    }

    pub fn table_registry(&self) -> Result<TableSchemaRegistry, ReadModelError> {
        let mut registry = TableSchemaRegistry::new();
        for schema in &self.tables {
            registry.register_schema(schema.clone())?;
        }
        Ok(registry)
    }

    pub fn sql_statements(&self, dialect: TableSqlDialect) -> Result<Vec<String>, ReadModelError> {
        table_schema_statements(&self.table_registry()?, dialect)
    }

    pub fn sql_migration_artifacts(
        &self,
        dialect: TableSqlDialect,
    ) -> Result<Vec<ReadModelMigrationArtifact>, ReadModelError> {
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
}

impl ServiceManifest {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            commands: Vec::new(),
            events: Vec::new(),
            transports: Vec::new(),
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
            .table_schema(outbox_message_schema())
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
}
