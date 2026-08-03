use serde::{Deserialize, Serialize};

use super::{
    generate_table_migration_artifacts, table_schema_statements, TableMigrationArtifact,
    TableSchema, TableSchemaRegistry, TableSqlDialect, TableStoreError,
};

/// A schema-only catalog for relational read models and operational tables.
///
/// This is deliberately not an application manifest. It owns physical table
/// metadata and SQL rendering only; application commands, projections,
/// surfaces, provenance, and executable mounts belong to
/// [`crate::application::ApplicationManifest`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadModelCatalog {
    pub name: String,
    pub tables: Vec<TableSchema>,
}

impl ReadModelCatalog {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            tables: Vec::new(),
        }
    }

    pub fn read_model<M>(mut self) -> Self
    where
        M: crate::read_model::RelationalReadModel,
    {
        self.try_register_read_model::<M>()
            .expect("read model schema should be valid in the schema catalog");
        self
    }

    pub fn try_read_model<M>(mut self) -> Result<Self, TableStoreError>
    where
        M: crate::read_model::RelationalReadModel,
    {
        self.try_register_read_model::<M>()?;
        Ok(self)
    }

    pub fn try_register_read_model<M>(&mut self) -> Result<&mut Self, TableStoreError>
    where
        M: crate::read_model::RelationalReadModel,
    {
        self.try_register_table_schema(M::schema().clone())
    }

    pub fn table_schema(mut self, schema: TableSchema) -> Self {
        self.try_register_table_schema(schema)
            .expect("table schema should be valid in the schema catalog");
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
        self.tables.sort_by(|left, right| {
            (left.model_name.as_str(), left.table_name.as_str())
                .cmp(&(right.model_name.as_str(), right.table_name.as_str()))
        });
        Ok(self)
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
}
