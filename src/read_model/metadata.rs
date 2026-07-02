use std::collections::{BTreeMap, BTreeSet};

use serde::{de::DeserializeOwned, Deserialize, Serialize};

use super::ReadModelError;

pub const DEFAULT_READ_MODEL_VERSION_COLUMN: &str = "_sourced_version";

/// Logical storage type for a relational read-model column.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ColumnType {
    Text,
    Boolean,
    Integer,
    UnsignedInteger,
    Float,
    Bytes,
    Json,
    Timestamp,
    Unsupported(String),
}

/// A foreign-key declaration from one read-model column to another table column.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForeignKey {
    pub table: String,
    pub column: String,
}

impl ForeignKey {
    pub fn new(table: impl Into<String>, column: impl Into<String>) -> Self {
        Self {
            table: table.into(),
            column: column.into(),
        }
    }
}

/// Primary-key metadata for a relational read-model table.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrimaryKey {
    pub columns: Vec<String>,
}

impl PrimaryKey {
    pub fn new(columns: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            columns: columns.into_iter().map(Into::into).collect(),
        }
    }
}

/// Runtime primary-key values for one read-model row.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RowKey {
    pub values: BTreeMap<String, RowValue>,
}

impl RowKey {
    pub fn new(values: impl IntoIterator<Item = (impl Into<String>, RowValue)>) -> Self {
        Self {
            values: values
                .into_iter()
                .map(|(column, value)| (column.into(), value))
                .collect(),
        }
    }

    pub fn insert(&mut self, column: impl Into<String>, value: RowValue) -> Option<RowValue> {
        self.values.insert(column.into(), value)
    }

    pub fn get(&self, column: &str) -> Option<&RowValue> {
        self.values.get(column)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &RowValue)> {
        self.values
            .iter()
            .map(|(column, value)| (column.as_str(), value))
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

/// Column metadata for a relational read-model table.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColumnDef {
    pub field_name: String,
    pub column_name: String,
    pub column_type: ColumnType,
    pub nullable: bool,
    pub has_default: bool,
    pub default: Option<String>,
    pub primary_key: bool,
    pub foreign_key: Option<ForeignKey>,
    pub delegated_from: Option<String>,
    pub jsonb: bool,
    pub skipped: bool,
}

impl ColumnDef {
    pub fn new(
        field_name: impl Into<String>,
        column_name: impl Into<String>,
        column_type: ColumnType,
    ) -> Self {
        Self {
            field_name: field_name.into(),
            column_name: column_name.into(),
            column_type,
            nullable: false,
            has_default: false,
            default: None,
            primary_key: false,
            foreign_key: None,
            delegated_from: None,
            jsonb: false,
            skipped: false,
        }
    }
}

/// Index or unique-constraint metadata for a relational read-model table.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexDef {
    pub name: Option<String>,
    pub columns: Vec<String>,
    pub unique: bool,
}

impl IndexDef {
    pub fn new(columns: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            name: None,
            columns: columns.into_iter().map(Into::into).collect(),
            unique: false,
        }
    }
}

/// Relationship category for later workspace/write-plan lowering.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RelationshipKind {
    HasMany,
    BelongsTo,
    ManyToMany,
}

/// Relationship metadata for a relational read model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelationshipDef {
    pub field_name: String,
    pub kind: RelationshipKind,
    pub target_model: String,
    pub foreign_key: Option<String>,
    pub through: Option<String>,
}

/// Schema metadata for one relational read-model table.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadModelSchema {
    pub model_name: String,
    pub table_name: String,
    pub columns: Vec<ColumnDef>,
    pub primary_key: PrimaryKey,
    pub version_column: Option<String>,
    pub foreign_keys: Vec<ForeignKey>,
    pub indexes: Vec<IndexDef>,
    pub relationships: Vec<RelationshipDef>,
}

impl ReadModelSchema {
    pub fn validate(&self) -> Result<(), ReadModelError> {
        if self.table_name.is_empty() {
            return Err(ReadModelError::Metadata(
                "read model schema must declare a table name".into(),
            ));
        }

        let mut columns = BTreeSet::new();
        for column in &self.columns {
            if column.column_name.is_empty() {
                return Err(ReadModelError::Metadata(format!(
                    "read model `{}` has a column with an empty name",
                    self.model_name
                )));
            }
            if !columns.insert(column.column_name.as_str()) {
                return Err(ReadModelError::Metadata(format!(
                    "read model `{}` declares duplicate column `{}`",
                    self.model_name, column.column_name
                )));
            }
            if let ColumnType::Unsupported(type_name) = &column.column_type {
                return Err(ReadModelError::Metadata(format!(
                    "read model `{}` field `{}` has unsupported field shape `{}`",
                    self.model_name, column.field_name, type_name
                )));
            }
            if let Some(foreign_key) = &column.foreign_key {
                validate_foreign_key(&self.model_name, foreign_key)?;
            }
        }

        if let Some(version_column) = &self.version_column {
            if version_column.is_empty() {
                return Err(ReadModelError::Metadata(format!(
                    "read model `{}` declares an empty version column",
                    self.model_name
                )));
            }
            if columns.contains(version_column.as_str()) {
                return Err(ReadModelError::Metadata(format!(
                    "read model `{}` version column `{}` conflicts with a mapped column",
                    self.model_name, version_column
                )));
            }
        }

        if self.primary_key.columns.is_empty() {
            return Err(ReadModelError::Metadata(format!(
                "read model `{}` must declare at least one primary-key column",
                self.model_name
            )));
        }

        for column in &self.primary_key.columns {
            if !columns.contains(column.as_str()) {
                return Err(ReadModelError::Metadata(format!(
                    "read model `{}` primary key references missing column `{}`",
                    self.model_name, column
                )));
            }
        }

        for foreign_key in &self.foreign_keys {
            validate_foreign_key(&self.model_name, foreign_key)?;
        }

        let mut index_names = BTreeSet::new();
        for index in &self.indexes {
            if let Some(name) = index.name.as_deref() {
                if !index_names.insert(name) {
                    return Err(ReadModelError::Metadata(format!(
                        "read model `{}` declares duplicate index name `{}`",
                        self.model_name, name
                    )));
                }
            }
            if index.columns.is_empty() {
                return Err(ReadModelError::Metadata(format!(
                    "read model `{}` declares an index with no columns",
                    self.model_name
                )));
            }
            for column in &index.columns {
                if !columns.contains(column.as_str()) {
                    return Err(ReadModelError::Metadata(format!(
                        "read model `{}` index references missing column `{}`",
                        self.model_name, column
                    )));
                }
            }
        }

        for relationship in &self.relationships {
            if relationship.target_model.is_empty() {
                return Err(ReadModelError::Metadata(format!(
                    "read model `{}` relationship `{}` must declare a target model",
                    self.model_name, relationship.field_name
                )));
            }
            if relationship
                .foreign_key
                .as_deref()
                .is_none_or(str::is_empty)
            {
                return Err(ReadModelError::Metadata(format!(
                    "read model `{}` relationship `{}` must declare a foreign key",
                    self.model_name, relationship.field_name
                )));
            }
        }

        Ok(())
    }
}

fn validate_foreign_key(model_name: &str, foreign_key: &ForeignKey) -> Result<(), ReadModelError> {
    if foreign_key.table.is_empty() || foreign_key.column.is_empty() {
        return Err(ReadModelError::Metadata(format!(
            "read model `{model_name}` has an invalid foreign-key declaration"
        )));
    }
    Ok(())
}

/// A typed value in a relational read-model row.
#[derive(Clone, Debug, PartialEq)]
pub enum RowValue {
    Null,
    Bool(bool),
    I64(i64),
    U64(u64),
    F64(f64),
    String(String),
    Bytes(Vec<u8>),
    Json(serde_json::Value),
}

impl RowValue {
    pub fn from_serde<T: Serialize + ?Sized>(value: &T) -> Result<Self, ReadModelError> {
        let value =
            serde_json::to_value(value).map_err(|err| ReadModelError::Serde(err.to_string()))?;
        Ok(Self::from_json_value(value))
    }

    pub fn into_json(self) -> serde_json::Value {
        match self {
            RowValue::Null => serde_json::Value::Null,
            RowValue::Bool(value) => serde_json::Value::Bool(value),
            RowValue::I64(value) => serde_json::Value::Number(value.into()),
            RowValue::U64(value) => serde_json::Value::Number(value.into()),
            RowValue::F64(value) => serde_json::json!(value),
            RowValue::String(value) => serde_json::Value::String(value),
            RowValue::Bytes(value) => serde_json::json!(value),
            RowValue::Json(value) => value,
        }
    }

    fn from_json_value(value: serde_json::Value) -> Self {
        match value {
            serde_json::Value::Null => RowValue::Null,
            serde_json::Value::Bool(value) => RowValue::Bool(value),
            serde_json::Value::Number(value) => {
                if let Some(value) = value.as_i64() {
                    RowValue::I64(value)
                } else if let Some(value) = value.as_u64() {
                    RowValue::U64(value)
                } else if let Some(value) = value.as_f64() {
                    RowValue::F64(value)
                } else {
                    RowValue::Json(serde_json::Value::Number(value))
                }
            }
            serde_json::Value::String(value) => RowValue::String(value),
            value @ (serde_json::Value::Array(_) | serde_json::Value::Object(_)) => {
                RowValue::Json(value)
            }
        }
    }
}

/// Column-value map for one relational read-model row.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RowValues {
    values: BTreeMap<String, RowValue>,
}

impl RowValues {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, column: impl Into<String>, value: RowValue) -> Option<RowValue> {
        self.values.insert(column.into(), value)
    }

    pub fn insert_serde<T: Serialize + ?Sized>(
        &mut self,
        column: impl Into<String>,
        value: &T,
    ) -> Result<Option<RowValue>, ReadModelError> {
        let value = RowValue::from_serde(value)?;
        Ok(self.insert(column, value))
    }

    pub fn get(&self, column: &str) -> Option<&RowValue> {
        self.values.get(column)
    }

    pub fn contains_key(&self, column: &str) -> bool {
        self.values.contains_key(column)
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn get_serde<T: DeserializeOwned>(&self, column: &str) -> Result<T, ReadModelError> {
        let value = self.values.get(column).ok_or_else(|| {
            ReadModelError::Metadata(format!("row is missing required column `{column}`"))
        })?;
        serde_json::from_value(value.clone().into_json())
            .map_err(|err| ReadModelError::Serde(err.to_string()))
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &RowValue)> {
        self.values
            .iter()
            .map(|(column, value)| (column.as_str(), value))
    }
}

impl IntoIterator for RowValues {
    type Item = (String, RowValue);
    type IntoIter = std::collections::btree_map::IntoIter<String, RowValue>;

    fn into_iter(self) -> Self::IntoIter {
        self.values.into_iter()
    }
}

/// Opt-in trait for table-mapped relational read models.
pub trait RelationalReadModel: Clone + Send + Sync + Sized {
    /// The model's schema. Static because a model's schema is fixed at compile
    /// time; the derive macro backs this with a `LazyLock` so staging mutations
    /// never rebuilds or clones schema metadata.
    fn schema() -> &'static ReadModelSchema;
    fn primary_key(&self) -> Result<RowKey, ReadModelError>;
    fn to_row(&self) -> Result<RowValues, ReadModelError>;
    fn from_row(row: RowValues) -> Result<Self, ReadModelError>;
}

/// Relationship hydration hooks generated for table-mapped read models.
pub trait RelationalReadModelIncludes: RelationalReadModel {
    fn hydrate_include(
        &mut self,
        include: &str,
        rows: Vec<RowValues>,
    ) -> Result<(), ReadModelError>;

    fn include_rows(&self, include: &str) -> Result<Vec<RowValues>, ReadModelError>;

    /// Schema of the model targeted by the named relationship.
    fn include_target_schema(include: &str) -> Result<&'static ReadModelSchema, ReadModelError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_schema() -> ReadModelSchema {
        ReadModelSchema {
            model_name: "PlayerWeapon".into(),
            table_name: "player_weapons".into(),
            columns: vec![
                ColumnDef {
                    primary_key: true,
                    foreign_key: Some(ForeignKey::new("players", "player_id")),
                    delegated_from: Some("Player.player_id".into()),
                    ..ColumnDef::new("player_id", "player_id", ColumnType::Text)
                },
                ColumnDef {
                    primary_key: true,
                    ..ColumnDef::new("weapon_id", "weapon_id", ColumnType::Text)
                },
            ],
            primary_key: PrimaryKey::new(["player_id", "weapon_id"]),
            version_column: Some(DEFAULT_READ_MODEL_VERSION_COLUMN.into()),
            foreign_keys: vec![ForeignKey::new("players", "player_id")],
            indexes: vec![IndexDef::new(["player_id"])],
            relationships: Vec::new(),
        }
    }

    #[test]
    fn validate_accepts_composite_delegated_key_metadata() {
        let schema = valid_schema();

        schema.validate().unwrap();
    }

    #[test]
    fn validate_rejects_missing_primary_key() {
        let mut schema = valid_schema();
        schema.primary_key = PrimaryKey::default();

        let err = schema.validate().unwrap_err();

        assert!(
            matches!(err, ReadModelError::Metadata(message) if message.contains("primary-key"))
        );
    }

    #[test]
    fn validate_rejects_unsupported_field_shapes() {
        let mut schema = valid_schema();
        schema.columns.push(ColumnDef::new(
            "callback",
            "callback",
            ColumnType::Unsupported("fn()".into()),
        ));

        let err = schema.validate().unwrap_err();

        assert!(
            matches!(err, ReadModelError::Metadata(message) if message.contains("unsupported field shape"))
        );
    }

    #[test]
    fn validate_rejects_relationships_without_foreign_keys() {
        let mut schema = valid_schema();
        schema.relationships.push(RelationshipDef {
            field_name: "weapons".into(),
            kind: RelationshipKind::HasMany,
            target_model: "PlayerWeapon".into(),
            foreign_key: None,
            through: None,
        });

        let err = schema.validate().unwrap_err();

        assert!(
            matches!(err, ReadModelError::Metadata(message) if message.contains("foreign key"))
        );
    }

    #[test]
    fn validate_rejects_duplicate_explicit_index_names() {
        let mut schema = valid_schema();
        schema.indexes = vec![
            IndexDef {
                name: Some("idx_player_weapons_player_id".into()),
                columns: vec!["player_id".into()],
                unique: false,
            },
            IndexDef {
                name: Some("idx_player_weapons_player_id".into()),
                columns: vec!["weapon_id".into()],
                unique: false,
            },
        ];

        let err = schema.validate().unwrap_err();

        assert!(matches!(err, ReadModelError::Metadata(message)
                if message.contains("duplicate index name `idx_player_weapons_player_id`")));
    }

    #[test]
    fn row_values_round_trip_scalar_and_json_values() {
        let mut row = RowValues::new();
        row.insert_serde("name", "Ada").unwrap();
        row.insert_serde("count", &3_i64).unwrap();
        row.insert_serde("payload", &serde_json::json!({"wins": [1, 2]}))
            .unwrap();

        assert_eq!(row.get_serde::<String>("name").unwrap(), "Ada");
        assert_eq!(row.get_serde::<i64>("count").unwrap(), 3);
        assert_eq!(
            row.get_serde::<serde_json::Value>("payload").unwrap(),
            serde_json::json!({"wins": [1, 2]})
        );
    }
}
