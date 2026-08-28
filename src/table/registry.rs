//! Registry and schema-management adapter surface for table schemas.

use std::collections::{BTreeMap, BTreeSet};

use crate::read_model::RelationalReadModel;

use super::{RelationshipDef, RelationshipKind, TableSchema, TableStoreError};

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
                    let through = relationship.through.as_deref().ok_or_else(|| {
                        TableStoreError::Metadata(format!(
                            "model `{}` relationship `{}` many-to-many must declare `through`",
                            schema.model_name, relationship.field_name
                        ))
                    })?;
                    let through_schema = self.schema_for_table(through).ok_or_else(|| {
                        TableStoreError::Metadata(format!(
                            "model `{}` relationship `{}` references unavailable join table `{}`",
                            schema.model_name, relationship.field_name, through
                        ))
                    })?;
                    let _ =
                        resolve_m2m_join_keys(schema, relationship, through_schema, target_schema)?;
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

/// One through-table column paired with one end-table primary-key column.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JoinColumnPair {
    pub through_column: String,
    pub end_column: String,
}

impl JoinColumnPair {
    pub fn new(through_column: impl Into<String>, end_column: impl Into<String>) -> Self {
        Self {
            through_column: through_column.into(),
            end_column: end_column.into(),
        }
    }
}

/// Through-table join keys for both ends of a many-to-many relationship.
///
/// `parent` pairs through columns with the source model's primary key.
/// `target` pairs through columns with the target model's primary key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct M2mJoinKeys {
    pub parent: Vec<JoinColumnPair>,
    pub target: Vec<JoinColumnPair>,
}

/// Resolve every through-column ↔ end-PK pair for a many-to-many relationship.
///
/// Through columns are either:
/// - the end's primary-key column names (present on the join table), or
/// - `foreign_key` / `target_foreign_key` listing through columns in PK order,
///   same arity as that end's primary key (comma-separated when more than one).
///
/// When those are absent, a unique through-column foreign-key reference to each
/// PK column is accepted. Compile SQL must not invent a second pairing.
pub fn resolve_m2m_join_keys(
    source: &TableSchema,
    relationship: &RelationshipDef,
    through_schema: &TableSchema,
    target_schema: &TableSchema,
) -> Result<M2mJoinKeys, TableStoreError> {
    if !matches!(relationship.kind, RelationshipKind::ManyToMany) {
        return Err(TableStoreError::Metadata(format!(
            "model `{}` relationship `{}` must be many-to-many to resolve join keys",
            source.model_name, relationship.field_name
        )));
    }
    Ok(M2mJoinKeys {
        parent: m2m_key_pairs(
            source,
            relationship,
            through_schema,
            source,
            "foreign_key",
            relationship.foreign_key.as_deref(),
        )?,
        target: m2m_key_pairs(
            source,
            relationship,
            through_schema,
            target_schema,
            "target_foreign_key",
            relationship.target_foreign_key.as_deref(),
        )?,
    })
}

fn m2m_key_pairs(
    source: &TableSchema,
    relationship: &RelationshipDef,
    through: &TableSchema,
    end: &TableSchema,
    field: &str,
    explicit: Option<&str>,
) -> Result<Vec<JoinColumnPair>, TableStoreError> {
    let pk_columns = &end.primary_key.columns;
    if pk_columns.is_empty() {
        return Err(TableStoreError::Metadata(format!(
            "model `{}` relationship `{}` cannot join through `{}` because `{}` has an empty primary key",
            source.model_name, relationship.field_name, through.table_name, end.model_name
        )));
    }
    if let Some(through_names) =
        parse_explicit_through_columns(source, relationship, field, explicit)?
    {
        if through_names.len() != pk_columns.len() {
            return Err(TableStoreError::Metadata(format!(
                "model `{}` relationship `{}` {field} lists {} through column(s) but `{}` primary key has {}",
                source.model_name,
                relationship.field_name,
                through_names.len(),
                end.model_name,
                pk_columns.len()
            )));
        }
        let mut pairs = Vec::with_capacity(pk_columns.len());
        for (through_name, end_column) in through_names.iter().zip(pk_columns) {
            let through_column = column_name_on(through, through_name).ok_or_else(|| {
                TableStoreError::Metadata(format!(
                    "model `{}` relationship `{}` {field} `{through_name}` is not a column on join table `{}`",
                    source.model_name, relationship.field_name, through.table_name
                ))
            })?;
            pairs.push(JoinColumnPair::new(through_column, end_column.clone()));
        }
        return Ok(pairs);
    }

    let mut pairs = Vec::with_capacity(pk_columns.len());
    let mut missing = Vec::new();
    for end_column in pk_columns {
        match column_name_on(through, end_column) {
            Some(through_column) => {
                pairs.push(JoinColumnPair::new(through_column, end_column.clone()));
            }
            None => missing.push(end_column.as_str()),
        }
    }
    if missing.is_empty() {
        return Ok(pairs);
    }
    if let Some(inferred) = infer_m2m_pairs_from_foreign_keys(through, end) {
        return Ok(inferred);
    }
    Err(TableStoreError::Metadata(format!(
        "model `{}` relationship `{}` cannot resolve {field} on join table `{}` for `{}` primary key [{}] \
         (missing same-named through columns: {}); declare `{field}` as a PK-order through-column list",
        source.model_name,
        relationship.field_name,
        through.table_name,
        end.model_name,
        pk_columns.join(", "),
        missing.join(", ")
    )))
}

fn parse_explicit_through_columns(
    source: &TableSchema,
    relationship: &RelationshipDef,
    field: &str,
    value: Option<&str>,
) -> Result<Option<Vec<String>>, TableStoreError> {
    let Some(raw) = value else {
        return Ok(None);
    };
    if raw.trim().is_empty() {
        return Err(TableStoreError::Metadata(format!(
            "model `{}` relationship `{}` {field} must not be empty",
            source.model_name, relationship.field_name
        )));
    }
    let mut columns = Vec::new();
    for part in raw.split(',') {
        let name = part.trim();
        if name.is_empty() {
            return Err(TableStoreError::Metadata(format!(
                "model `{}` relationship `{}` {field} lists an empty through column",
                source.model_name, relationship.field_name
            )));
        }
        columns.push(name.to_string());
    }
    Ok(Some(columns))
}

fn infer_m2m_pairs_from_foreign_keys(
    through: &TableSchema,
    end: &TableSchema,
) -> Option<Vec<JoinColumnPair>> {
    let mut pairs = Vec::with_capacity(end.primary_key.columns.len());
    for end_column in &end.primary_key.columns {
        let matches: Vec<&str> = through
            .columns
            .iter()
            .filter(|column| {
                column
                    .foreign_key
                    .as_ref()
                    .is_some_and(|fk| fk.table == end.table_name && fk.column == *end_column)
            })
            .map(|column| column.column_name.as_str())
            .collect();
        let [only] = matches.as_slice() else {
            return None;
        };
        pairs.push(JoinColumnPair::new(*only, end_column.clone()));
    }
    Some(pairs)
}

fn column_name_on<'a>(schema: &'a TableSchema, name: &str) -> Option<&'a str> {
    schema.columns.iter().find_map(|column| {
        if column.column_name == name || column.field_name == name {
            Some(column.column_name.as_str())
        } else {
            None
        }
    })
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

#[cfg(test)]
mod m2m_join_key_tests {
    use super::*;
    use crate::table::{
        ColumnType, ForeignKey, PrimaryKey, RelationshipDef, RelationshipKind, TableColumn,
        TableKind, TableSchema,
    };

    fn pk_column(name: &str) -> TableColumn {
        TableColumn {
            primary_key: true,
            ..TableColumn::new(name, name, ColumnType::Text)
        }
    }

    fn column(name: &str) -> TableColumn {
        TableColumn::new(name, name, ColumnType::Text)
    }

    fn schema(
        model: &str,
        table: &str,
        columns: Vec<TableColumn>,
        pk: &[&str],
        relationships: Vec<RelationshipDef>,
    ) -> TableSchema {
        TableSchema {
            model_name: model.into(),
            table_name: table.into(),
            columns,
            primary_key: PrimaryKey::new(pk.iter().copied()),
            version_column: None,
            foreign_keys: Vec::new(),
            indexes: Vec::new(),
            relationships,
            kind: TableKind::ReadModel,
        }
    }

    fn labels() -> TableSchema {
        schema(
            "LabelView",
            "labels",
            vec![pk_column("label_id"), column("name")],
            &["label_id"],
            Vec::new(),
        )
    }

    fn projects(rel: RelationshipDef) -> TableSchema {
        schema(
            "ProjectView",
            "projects",
            vec![pk_column("workspace_id"), pk_column("path"), column("kind")],
            &["workspace_id", "path"],
            vec![rel],
        )
    }

    fn project_labels() -> TableSchema {
        schema(
            "ProjectLabel",
            "project_labels",
            vec![
                pk_column("workspace_id"),
                pk_column("path"),
                pk_column("label_id"),
            ],
            &["workspace_id", "path", "label_id"],
            Vec::new(),
        )
    }

    fn labels_rel(foreign_key: Option<&str>, target_foreign_key: Option<&str>) -> RelationshipDef {
        RelationshipDef {
            field_name: "labels".into(),
            kind: RelationshipKind::ManyToMany,
            target_model: "LabelView".into(),
            foreign_key: foreign_key.map(str::to_string),
            through: Some("project_labels".into()),
            target_foreign_key: target_foreign_key.map(str::to_string),
        }
    }

    #[test]
    fn same_named_through_columns_pair_composite_and_single_keys() {
        let source = projects(labels_rel(None, None));
        let keys = resolve_m2m_join_keys(
            &source,
            &source.relationships[0],
            &project_labels(),
            &labels(),
        )
        .unwrap();
        assert_eq!(
            keys.parent,
            vec![
                JoinColumnPair::new("workspace_id", "workspace_id"),
                JoinColumnPair::new("path", "path"),
            ]
        );
        assert_eq!(
            keys.target,
            vec![JoinColumnPair::new("label_id", "label_id")]
        );
    }

    #[test]
    fn explicit_pk_order_list_renames_composite_through_columns() {
        let mut through = project_labels();
        through.columns = vec![
            pk_column("project_workspace_id"),
            pk_column("project_path"),
            pk_column("tag_id"),
        ];
        through.primary_key = PrimaryKey::new(["project_workspace_id", "project_path", "tag_id"]);
        let source = projects(labels_rel(
            Some("project_workspace_id,project_path"),
            Some("tag_id"),
        ));
        let keys =
            resolve_m2m_join_keys(&source, &source.relationships[0], &through, &labels()).unwrap();
        assert_eq!(
            keys.parent,
            vec![
                JoinColumnPair::new("project_workspace_id", "workspace_id"),
                JoinColumnPair::new("project_path", "path"),
            ]
        );
        assert_eq!(keys.target, vec![JoinColumnPair::new("tag_id", "label_id")]);
    }

    #[test]
    fn partial_explicit_foreign_key_does_not_silently_ignore_composite_pk() {
        let source = projects(labels_rel(Some("workspace_id"), Some("label_id")));
        let error = resolve_m2m_join_keys(
            &source,
            &source.relationships[0],
            &project_labels(),
            &labels(),
        )
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("lists 1 through column") && error.contains("primary key has 2"),
            "{error}"
        );
    }

    #[test]
    fn empty_string_foreign_key_is_not_a_missing_sentinel() {
        let source = projects(labels_rel(Some(""), None));
        let error = resolve_m2m_join_keys(
            &source,
            &source.relationships[0],
            &project_labels(),
            &labels(),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("foreign_key must not be empty"), "{error}");
    }

    #[test]
    fn infers_renamed_through_columns_from_column_foreign_keys() {
        let mut through = project_labels();
        through.columns = vec![
            TableColumn {
                primary_key: true,
                foreign_key: Some(ForeignKey::new("projects", "workspace_id")),
                ..TableColumn::new(
                    "project_workspace_id",
                    "project_workspace_id",
                    ColumnType::Text,
                )
            },
            TableColumn {
                primary_key: true,
                foreign_key: Some(ForeignKey::new("projects", "path")),
                ..TableColumn::new("project_path", "project_path", ColumnType::Text)
            },
            TableColumn {
                primary_key: true,
                foreign_key: Some(ForeignKey::new("labels", "label_id")),
                ..TableColumn::new("tag_id", "tag_id", ColumnType::Text)
            },
        ];
        through.primary_key = PrimaryKey::new(["project_workspace_id", "project_path", "tag_id"]);
        let source = projects(labels_rel(None, None));
        let keys =
            resolve_m2m_join_keys(&source, &source.relationships[0], &through, &labels()).unwrap();
        assert_eq!(
            keys.parent,
            vec![
                JoinColumnPair::new("project_workspace_id", "workspace_id"),
                JoinColumnPair::new("project_path", "path"),
            ]
        );
        assert_eq!(keys.target, vec![JoinColumnPair::new("tag_id", "label_id")]);
    }
}
