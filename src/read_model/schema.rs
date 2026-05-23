use std::collections::{BTreeMap, BTreeSet};

use super::{ReadModelError, ReadModelSchema, RelationalReadModel};

/// Registry of table-mapped read-model schemas an adapter should manage.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReadModelSchemaRegistry {
    schemas_by_table: BTreeMap<String, ReadModelSchema>,
    tables_by_model: BTreeMap<String, String>,
}

impl ReadModelSchemaRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<M>(&mut self) -> Result<&mut Self, ReadModelError>
    where
        M: RelationalReadModel,
    {
        self.register_schema(M::schema())
    }

    pub fn register_schema(
        &mut self,
        schema: ReadModelSchema,
    ) -> Result<&mut Self, ReadModelError> {
        schema.validate()?;

        if self.schemas_by_table.contains_key(&schema.table_name) {
            return Err(ReadModelError::Metadata(format!(
                "read-model schema registry already contains table `{}`",
                schema.table_name
            )));
        }
        if self.tables_by_model.contains_key(&schema.model_name) {
            return Err(ReadModelError::Metadata(format!(
                "read-model schema registry already contains model `{}`",
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

    pub fn schemas(&self) -> impl Iterator<Item = &ReadModelSchema> {
        self.schemas_by_table.values()
    }

    pub fn table_names(&self) -> impl Iterator<Item = &str> {
        self.schemas_by_table.keys().map(String::as_str)
    }

    pub fn schema_for_table(&self, table_name: &str) -> Option<&ReadModelSchema> {
        self.schemas_by_table.get(table_name)
    }

    pub fn schema_for_model(&self, model_name: &str) -> Option<&ReadModelSchema> {
        self.tables_by_model
            .get(model_name)
            .and_then(|table_name| self.schema_for_table(table_name))
    }

    pub fn validate(&self) -> Result<(), ReadModelError> {
        let table_names = self
            .schemas_by_table
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        let model_names = self
            .tables_by_model
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();

        for schema in self.schemas() {
            schema.validate()?;
            self.validate_column_foreign_keys(schema, &table_names)?;
            self.validate_schema_foreign_keys(schema, &table_names)?;
            self.validate_relationships(schema, &model_names, &table_names)?;
        }

        Ok(())
    }

    fn validate_column_foreign_keys(
        &self,
        schema: &ReadModelSchema,
        table_names: &BTreeSet<String>,
    ) -> Result<(), ReadModelError> {
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
        schema: &ReadModelSchema,
        table_names: &BTreeSet<String>,
    ) -> Result<(), ReadModelError> {
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
        schema: &ReadModelSchema,
        model_names: &BTreeSet<String>,
        table_names: &BTreeSet<String>,
    ) -> Result<(), ReadModelError> {
        for relationship in &schema.relationships {
            if !model_names.contains(&relationship.target_model) {
                return Err(ReadModelError::Metadata(format!(
                    "read model `{}` relationship `{}` targets unregistered model `{}`",
                    schema.model_name, relationship.field_name, relationship.target_model
                )));
            }

            if let Some(through) = relationship.through.as_deref() {
                if !table_names.contains(through) {
                    return Err(ReadModelError::Metadata(format!(
                        "read model `{}` relationship `{}` references unregistered join table `{}`",
                        schema.model_name, relationship.field_name, through
                    )));
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
    ) -> Result<(), ReadModelError> {
        if !table_names.contains(target_table) {
            return Err(ReadModelError::Metadata(format!(
                "read model `{model_name}` table `{table_name}` references unregistered foreign-key table `{target_table}`"
            )));
        }

        let target_schema = self.schemas_by_table.get(target_table).ok_or_else(|| {
            ReadModelError::Metadata(format!(
                "read model `{model_name}` references unavailable foreign-key table `{target_table}`"
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
            return Err(ReadModelError::Metadata(format!(
                "read model `{model_name}` {local_column} references missing foreign-key column `{target_table}.{target_column}`"
            )));
        }

        Ok(())
    }
}

/// Schema lifecycle operations an adapter can support.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReadModelSchemaAdapterCapabilities {
    pub migration_artifacts: bool,
    pub schema_verification: bool,
    pub dev_bootstrap: bool,
}

impl ReadModelSchemaAdapterCapabilities {
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
pub struct ReadModelMigrationArtifact {
    pub name: String,
    pub statements: Vec<String>,
}

impl ReadModelMigrationArtifact {
    pub fn new(name: impl Into<String>, statements: impl IntoIterator<Item = String>) -> Self {
        Self {
            name: name.into(),
            statements: statements.into_iter().collect(),
        }
    }
}

/// Result of verifying registered metadata against an adapter-owned schema.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReadModelSchemaVerification {
    pub issues: Vec<ReadModelSchemaIssue>,
}

impl ReadModelSchemaVerification {
    pub fn verified() -> Self {
        Self::default()
    }

    pub fn is_verified(&self) -> bool {
        self.issues.is_empty()
    }
}

/// Adapter-facing schema verification issue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadModelSchemaIssue {
    pub table_name: String,
    pub column_name: Option<String>,
    pub kind: ReadModelSchemaIssueKind,
    pub message: String,
}

impl ReadModelSchemaIssue {
    pub fn new(
        table_name: impl Into<String>,
        column_name: Option<impl Into<String>>,
        kind: ReadModelSchemaIssueKind,
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
pub enum ReadModelSchemaIssueKind {
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
pub struct ReadModelSchemaBootstrap {
    pub bootstrapped_tables: Vec<String>,
}

impl ReadModelSchemaBootstrap {
    pub fn new(bootstrapped_tables: impl IntoIterator<Item = String>) -> Self {
        Self {
            bootstrapped_tables: bootstrapped_tables.into_iter().collect(),
        }
    }
}

/// Adapter contract for schema generation, verification, and dev/test bootstrap.
pub trait ReadModelSchemaAdapter {
    fn schema_capabilities(&self) -> ReadModelSchemaAdapterCapabilities;

    fn generate_migration_artifacts(
        &self,
        _registry: &ReadModelSchemaRegistry,
    ) -> Result<Vec<ReadModelMigrationArtifact>, ReadModelError> {
        Err(ReadModelError::Metadata(
            "read-model schema adapter does not support migration artifact generation".into(),
        ))
    }

    fn verify_schema(
        &self,
        _registry: &ReadModelSchemaRegistry,
    ) -> Result<ReadModelSchemaVerification, ReadModelError> {
        Err(ReadModelError::Metadata(
            "read-model schema adapter does not support startup schema verification".into(),
        ))
    }

    fn bootstrap_schema_for_dev(
        &self,
        _registry: &ReadModelSchemaRegistry,
    ) -> Result<ReadModelSchemaBootstrap, ReadModelError> {
        Err(ReadModelError::Metadata(
            "read-model schema adapter does not support explicit dev/test bootstrap".into(),
        ))
    }
}
