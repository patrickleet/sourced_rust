//! Registry and schema-management adapter surface for table schemas.

use std::collections::{BTreeMap, BTreeSet};

use crate::read_model::RelationalReadModel;

use super::{RelationshipKind, TableSchema, TableStoreError};

/// Registry of table schemas an adapter should manage.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TableSchemaRegistry {
    schemas_by_table: BTreeMap<String, TableSchema>,
    tables_by_model: BTreeMap<String, String>,
}

impl TableSchemaRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<M>(&mut self) -> Result<&mut Self, TableStoreError>
    where
        M: RelationalReadModel,
    {
        self.register_schema(M::schema().clone())
    }

    pub fn register_schema(&mut self, schema: TableSchema) -> Result<&mut Self, TableStoreError> {
        schema.validate()?;

        if self.schemas_by_table.contains_key(&schema.table_name) {
            return Err(TableStoreError::Metadata(format!(
                "table schema registry already contains table `{}`",
                schema.table_name
            )));
        }
        if self.tables_by_model.contains_key(&schema.model_name) {
            return Err(TableStoreError::Metadata(format!(
                "table schema registry already contains model `{}`",
                schema.model_name
            )));
        }

        self.tables_by_model
            .insert(schema.model_name.clone(), schema.table_name.clone());
        self.schemas_by_table
            .insert(schema.table_name.clone(), schema);
        Ok(self)
    }

    pub fn len(&self) -> usize {
        self.schemas_by_table.len()
    }

    pub fn is_empty(&self) -> bool {
        self.schemas_by_table.is_empty()
    }

    pub fn schemas(&self) -> impl Iterator<Item = &TableSchema> {
        self.schemas_by_table.values()
    }

    pub fn table_names(&self) -> impl Iterator<Item = &str> {
        self.schemas_by_table.keys().map(String::as_str)
    }

    pub fn schema_for_table(&self, table_name: &str) -> Option<&TableSchema> {
        self.schemas_by_table.get(table_name)
    }

    pub fn schema_for_model(&self, model_name: &str) -> Option<&TableSchema> {
        self.tables_by_model
            .get(model_name)
            .and_then(|table_name| self.schema_for_table(table_name))
    }

    pub fn validate(&self) -> Result<(), TableStoreError> {
        let table_names = self
            .schemas_by_table
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        for schema in self.schemas() {
            schema.validate()?;
            self.validate_column_foreign_keys(schema, &table_names)?;
            self.validate_schema_foreign_keys(schema, &table_names)?;
            self.validate_relationships(schema, &table_names)?;
        }

        Ok(())
    }

    fn validate_column_foreign_keys(
        &self,
        schema: &TableSchema,
        table_names: &BTreeSet<String>,
    ) -> Result<(), TableStoreError> {
        for column in &schema.columns {
            let Some(foreign_key) = &column.foreign_key else {
                continue;
            };
            self.validate_foreign_key_target(
                &schema.model_name,
                &schema.table_name,
                &column.column_name,
                &foreign_key.table,
                &foreign_key.column,
                table_names,
            )?;
        }
        Ok(())
    }

    fn validate_schema_foreign_keys(
        &self,
        schema: &TableSchema,
        table_names: &BTreeSet<String>,
    ) -> Result<(), TableStoreError> {
        for foreign_key in &schema.foreign_keys {
            self.validate_foreign_key_target(
                &schema.model_name,
                &schema.table_name,
                "",
                &foreign_key.table,
                &foreign_key.column,
                table_names,
            )?;
        }
        Ok(())
    }

    fn validate_relationships(
        &self,
        schema: &TableSchema,
        table_names: &BTreeSet<String>,
    ) -> Result<(), TableStoreError> {
        for relationship in &schema.relationships {
            let target_schema = self
                .schema_for_model(&relationship.target_model)
                .ok_or_else(|| {
                    TableStoreError::Metadata(format!(
                        "model `{}` relationship `{}` targets unregistered model `{}`",
                        schema.model_name, relationship.field_name, relationship.target_model
                    ))
                })?;

            if let Some(through) = relationship.through.as_deref() {
                if !table_names.contains(through) {
                    return Err(TableStoreError::Metadata(format!(
                        "model `{}` relationship `{}` references unregistered join table `{}`",
                        schema.model_name, relationship.field_name, through
                    )));
                }
            }

            let foreign_key = relationship.foreign_key.as_deref().unwrap_or_default();
            match relationship.kind {
                RelationshipKind::HasMany => {
                    if !schema_has_column_or_field(target_schema, foreign_key) {
                        return Err(TableStoreError::Metadata(format!(
                            "model `{}` relationship `{}` foreign key `{}` is not a column on target model `{}`",
                            schema.model_name,
                            relationship.field_name,
                            foreign_key,
                            relationship.target_model
                        )));
                    }
                }
                RelationshipKind::BelongsTo => {
                    if !schema_has_column_or_field(schema, foreign_key) {
                        return Err(TableStoreError::Metadata(format!(
                            "model `{}` relationship `{}` foreign key `{}` is not a column on source model `{}`",
                            schema.model_name,
                            relationship.field_name,
                            foreign_key,
                            schema.model_name
                        )));
                    }
                }
                RelationshipKind::ManyToMany => {
                    if let Some(through) = relationship.through.as_deref() {
                        let through_schema = self.schema_for_table(through).ok_or_else(|| {
                            TableStoreError::Metadata(format!(
                                "model `{}` relationship `{}` references unavailable join table `{}`",
                                schema.model_name, relationship.field_name, through
                            ))
                        })?;
                        if !schema_has_column_or_field(through_schema, foreign_key) {
                            return Err(TableStoreError::Metadata(format!(
                                "model `{}` relationship `{}` foreign key `{}` is not a column on join table `{}`",
                                schema.model_name,
                                relationship.field_name,
                                foreign_key,
                                through
                            )));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn validate_foreign_key_target(
        &self,
        model_name: &str,
        table_name: &str,
        column_name: &str,
        target_table: &str,
        target_column: &str,
        table_names: &BTreeSet<String>,
    ) -> Result<(), TableStoreError> {
        if !table_names.contains(target_table) {
            return Err(TableStoreError::Metadata(format!(
                "model `{model_name}` table `{table_name}` references unregistered foreign-key table `{target_table}`"
            )));
        }

        let target_schema = self.schemas_by_table.get(target_table).ok_or_else(|| {
            TableStoreError::Metadata(format!(
                "model `{model_name}` references unavailable foreign-key table `{target_table}`"
            ))
        })?;
        if !target_schema
            .columns
            .iter()
            .any(|column| column.column_name == target_column)
        {
            let local_column = if column_name.is_empty() {
                "schema".to_string()
            } else {
                format!("column `{column_name}`")
            };
            return Err(TableStoreError::Metadata(format!(
                "model `{model_name}` {local_column} references missing foreign-key column `{target_table}.{target_column}`"
            )));
        }

        Ok(())
    }
}

fn schema_has_column_or_field(schema: &TableSchema, name: &str) -> bool {
    schema
        .columns
        .iter()
        .any(|column| column.column_name == name || column.field_name == name)
}

/// Schema lifecycle operations an adapter can support.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TableSchemaAdapterCapabilities {
    pub migration_artifacts: bool,
    pub schema_verification: bool,
    pub dev_bootstrap: bool,
}

impl TableSchemaAdapterCapabilities {
    pub fn all() -> Self {
        Self {
            migration_artifacts: true,
            schema_verification: true,
            dev_bootstrap: true,
        }
    }
}

/// Generated or user-consumable migration artifact for registered schemas.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableMigrationArtifact {
    pub name: String,
    pub statements: Vec<String>,
}

impl TableMigrationArtifact {
    pub fn new(name: impl Into<String>, statements: impl IntoIterator<Item = String>) -> Self {
        Self {
            name: name.into(),
            statements: statements.into_iter().collect(),
        }
    }
}

/// Result of verifying registered metadata against an adapter-owned schema.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TableSchemaVerification {
    pub issues: Vec<TableSchemaIssue>,
}

impl TableSchemaVerification {
    pub fn verified() -> Self {
        Self::default()
    }

    pub fn is_verified(&self) -> bool {
        self.issues.is_empty()
    }
}

/// Adapter-facing schema verification issue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableSchemaIssue {
    pub table_name: String,
    pub column_name: Option<String>,
    pub kind: TableSchemaIssueKind,
    pub message: String,
}

impl TableSchemaIssue {
    pub fn new(
        table_name: impl Into<String>,
        column_name: Option<impl Into<String>>,
        kind: TableSchemaIssueKind,
        message: impl Into<String>,
    ) -> Self {
        Self {
            table_name: table_name.into(),
            column_name: column_name.map(Into::into),
            kind,
            message: message.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TableSchemaIssueKind {
    MissingTable,
    MissingColumn,
    TypeMismatch,
    PrimaryKeyMismatch,
    ForeignKeyMismatch,
    IndexMismatch,
    NullabilityMismatch,
    DefaultMismatch,
    VersionColumnMismatch,
    Unsupported(String),
}

/// Result of an explicit dev/test schema bootstrap operation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TableSchemaBootstrap {
    pub bootstrapped_tables: Vec<String>,
}

impl TableSchemaBootstrap {
    pub fn new(bootstrapped_tables: impl IntoIterator<Item = String>) -> Self {
        Self {
            bootstrapped_tables: bootstrapped_tables.into_iter().collect(),
        }
    }
}

/// Adapter contract for schema generation, verification, and dev/test bootstrap.
pub trait TableSchemaAdapter {
    fn schema_capabilities(&self) -> TableSchemaAdapterCapabilities;

    fn generate_migration_artifacts(
        &self,
        _registry: &TableSchemaRegistry,
    ) -> Result<Vec<TableMigrationArtifact>, TableStoreError> {
        Err(TableStoreError::Metadata(
            "read-model schema adapter does not support migration artifact generation".into(),
        ))
    }

    fn verify_schema(
        &self,
        _registry: &TableSchemaRegistry,
    ) -> Result<TableSchemaVerification, TableStoreError> {
        Err(TableStoreError::Metadata(
            "read-model schema adapter does not support startup schema verification".into(),
        ))
    }

    fn bootstrap_schema_for_dev(
        &self,
        _registry: &TableSchemaRegistry,
    ) -> Result<TableSchemaBootstrap, TableStoreError> {
        Err(TableStoreError::Metadata(
            "read-model schema adapter does not support explicit dev/test bootstrap".into(),
        ))
    }
}
