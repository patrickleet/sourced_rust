//! Staged relational row mutations and the row/key validation helpers shared
//! by the write-plan builder, the workspace, and the storage adapters.

use std::cmp::Ordering;

use serde::Serialize;

use super::{ReadModelError, ReadModelSchema, RowKey, RowValue, RowValues};

/// Expected optimistic version carried by a staged read-model write.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ExpectedVersion {
    /// No optimistic version check is requested.
    #[default]
    Any,
    /// The target row must currently have this version.
    Exact(u64),
    /// The target row must not exist yet.
    NotExists,
}

/// Full-row write behavior for a relational row mutation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowWriteMode {
    Insert,
    Upsert,
}

/// Sparse patch behavior for a relational row mutation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PatchMode {
    UpdateExisting,
    InsertMissing,
}

/// Sparse column updates for a relational row.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RowPatch {
    values: RowValues,
}

impl RowPatch {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(mut self, column: impl Into<String>, value: RowValue) -> Self {
        self.values.insert(column, value);
        self
    }

    pub fn set_serde<T: Serialize + ?Sized>(
        mut self,
        column: impl Into<String>,
        value: &T,
    ) -> Result<Self, ReadModelError> {
        self.values.insert_serde(column, value)?;
        Ok(self)
    }

    pub fn get(&self, column: &str) -> Option<&RowValue> {
        self.values.get(column)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &RowValue)> {
        self.values.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn into_values(self) -> RowValues {
        self.values
    }
}

/// Full relational row insert/upsert mutation.
#[derive(Clone, Debug, PartialEq)]
pub struct RowMutation {
    pub schema: &'static ReadModelSchema,
    pub key: RowKey,
    pub values: RowValues,
    pub expected_version: ExpectedVersion,
    pub mode: RowWriteMode,
}

/// Sparse relational row patch mutation.
#[derive(Clone, Debug, PartialEq)]
pub struct PatchRowMutation {
    pub schema: &'static ReadModelSchema,
    pub key: RowKey,
    pub patch: RowPatch,
    pub expected_version: ExpectedVersion,
    pub mode: PatchMode,
}

/// Relational row delete mutation.
#[derive(Clone, Debug, PartialEq)]
pub struct DeleteRowMutation {
    pub schema: &'static ReadModelSchema,
    pub key: RowKey,
    pub expected_version: ExpectedVersion,
}

/// First-pass read-model write-plan mutation surface.
#[derive(Clone, Debug, PartialEq)]
pub enum ReadModelMutation {
    UpsertRow(RowMutation),
    PatchRow(PatchRowMutation),
    DeleteRow(DeleteRowMutation),
}

impl ReadModelMutation {
    pub fn table_name(&self) -> &str {
        self.schema().table_name.as_str()
    }

    pub fn lock_key(&self) -> String {
        format!("{}:{}", self.table_name(), key_fingerprint(self.key()))
    }

    fn key(&self) -> &RowKey {
        match self {
            ReadModelMutation::UpsertRow(mutation) => &mutation.key,
            ReadModelMutation::PatchRow(mutation) => &mutation.key,
            ReadModelMutation::DeleteRow(mutation) => &mutation.key,
        }
    }

    pub(super) fn operation_rank(&self) -> u8 {
        match self {
            ReadModelMutation::UpsertRow(_) => 1,
            ReadModelMutation::PatchRow(_) => 2,
            ReadModelMutation::DeleteRow(_) => 3,
        }
    }

    fn schema(&self) -> &'static ReadModelSchema {
        match self {
            ReadModelMutation::UpsertRow(mutation) => mutation.schema,
            ReadModelMutation::PatchRow(mutation) => mutation.schema,
            ReadModelMutation::DeleteRow(mutation) => mutation.schema,
        }
    }

    fn depends_on_table(&self, table_name: &str) -> bool {
        let schema = self.schema();
        schema
            .foreign_keys
            .iter()
            .any(|foreign_key| foreign_key.table == table_name)
            || schema.columns.iter().any(|column| {
                column
                    .foreign_key
                    .as_ref()
                    .is_some_and(|foreign_key| foreign_key.table == table_name)
            })
    }

    pub(super) fn dependency_order(&self, other: &Self) -> Option<Ordering> {
        let self_depends_on_other = self.depends_on_table(other.table_name());
        let other_depends_on_self = other.depends_on_table(self.table_name());

        match (self_depends_on_other, other_depends_on_self) {
            (true, false) if self.operation_rank() == 3 && other.operation_rank() == 3 => {
                Some(Ordering::Less)
            }
            (true, false) => Some(Ordering::Greater),
            (false, true) if self.operation_rank() == 3 && other.operation_rank() == 3 => {
                Some(Ordering::Greater)
            }
            (false, true) => Some(Ordering::Less),
            _ => None,
        }
    }

    pub(super) fn sort_key(&self) -> String {
        format!(
            "{}|{}|{}",
            self.operation_rank(),
            self.table_name(),
            key_fingerprint(self.key())
        )
    }
}

pub(super) fn validate_row_mutation(mutation: &RowMutation) -> Result<(), ReadModelError> {
    mutation.schema.validate()?;
    validate_key(mutation.schema, &mutation.key)?;
    validate_expected_version(&mutation.expected_version, mutation.schema)?;
    validate_row_values(mutation.schema, &mutation.values, true)
}

pub(super) fn validate_patch_mutation(mutation: &PatchRowMutation) -> Result<(), ReadModelError> {
    mutation.schema.validate()?;
    validate_key(mutation.schema, &mutation.key)?;
    validate_expected_version(&mutation.expected_version, mutation.schema)?;
    if mutation.patch.is_empty() {
        return Err(ReadModelError::Metadata(format!(
            "read model `{}` patch must set at least one column",
            mutation.schema.model_name
        )));
    }
    validate_row_values(mutation.schema, &mutation.patch.values, false)
}

pub(super) fn validate_delete_mutation(mutation: &DeleteRowMutation) -> Result<(), ReadModelError> {
    mutation.schema.validate()?;
    validate_key(mutation.schema, &mutation.key)?;
    validate_expected_version(&mutation.expected_version, mutation.schema)
}

pub(super) fn validate_expected_version(
    expected_version: &ExpectedVersion,
    schema: &ReadModelSchema,
) -> Result<(), ReadModelError> {
    if matches!(expected_version, ExpectedVersion::Exact(0)) {
        return Err(ReadModelError::Metadata(format!(
            "read model `{}` expected version must be greater than zero",
            schema.model_name
        )));
    }
    Ok(())
}

pub(crate) fn validate_key(schema: &ReadModelSchema, key: &RowKey) -> Result<(), ReadModelError> {
    if key.is_empty() {
        return Err(ReadModelError::Metadata(format!(
            "read model `{}` row key cannot be empty",
            schema.model_name
        )));
    }

    for column in &schema.primary_key.columns {
        match key.get(column) {
            Some(RowValue::Null) => {
                return Err(ReadModelError::Metadata(format!(
                    "read model `{}` primary-key column `{}` cannot be null",
                    schema.model_name, column
                )));
            }
            Some(_) => {}
            None => {
                return Err(ReadModelError::Metadata(format!(
                    "read model `{}` row key is missing primary-key column `{}`",
                    schema.model_name, column
                )));
            }
        }
    }

    for (column, _) in key.iter() {
        if !schema.primary_key.columns.iter().any(|key| key == column) {
            return Err(ReadModelError::Metadata(format!(
                "read model `{}` row key includes non-primary-key column `{}`",
                schema.model_name, column
            )));
        }
    }

    Ok(())
}

pub(crate) fn validate_row_values(
    schema: &ReadModelSchema,
    values: &RowValues,
    full_row: bool,
) -> Result<(), ReadModelError> {
    for (column_name, value) in values.iter() {
        let column = schema
            .columns
            .iter()
            .find(|column| column.column_name == column_name)
            .ok_or_else(|| {
                ReadModelError::Metadata(format!(
                    "read model `{}` write references missing column `{}`",
                    schema.model_name, column_name
                ))
            })?;

        if matches!(value, RowValue::Null) {
            if column.primary_key {
                return Err(ReadModelError::Metadata(format!(
                    "read model `{}` primary-key column `{}` cannot be null",
                    schema.model_name, column.column_name
                )));
            }
            if !column.nullable && !column.has_default {
                return Err(ReadModelError::Metadata(format!(
                    "read model `{}` column `{}` is not nullable",
                    schema.model_name, column.column_name
                )));
            }
        }
    }

    if full_row {
        for column in &schema.columns {
            if column.skipped || column.nullable || column.has_default {
                continue;
            }
            if !values.contains_key(&column.column_name) {
                return Err(ReadModelError::Metadata(format!(
                    "read model `{}` row is missing required column `{}`",
                    schema.model_name, column.column_name
                )));
            }
        }

        for column in schema
            .columns
            .iter()
            .filter(|column| column.delegated_from.is_some())
        {
            match values.get(&column.column_name) {
                Some(RowValue::Null) | None => {
                    return Err(ReadModelError::Metadata(format!(
                        "read model `{}` delegated column `{}` must be populated before write",
                        schema.model_name, column.column_name
                    )));
                }
                Some(_) => {}
            }
        }
    }

    Ok(())
}

pub(crate) fn key_from_row(
    schema: &ReadModelSchema,
    row: &RowValues,
) -> Result<RowKey, ReadModelError> {
    let mut key = RowKey::default();
    for column in &schema.primary_key.columns {
        let value = row.get(column).cloned().ok_or_else(|| {
            ReadModelError::Metadata(format!(
                "read model `{}` row is missing primary-key column `{}`",
                schema.model_name, column
            ))
        })?;
        key.insert(column.clone(), value);
    }
    validate_key(schema, &key)?;
    Ok(key)
}

pub(crate) fn column_name_for(schema: &ReadModelSchema, field_or_column: &str) -> Option<String> {
    schema
        .columns
        .iter()
        .find(|column| {
            column.field_name == field_or_column || column.column_name == field_or_column
        })
        .map(|column| column.column_name.clone())
}

pub(crate) fn key_fingerprint(key: &RowKey) -> String {
    let mut fingerprint = String::new();
    for (column, value) in key.iter() {
        push_fingerprint_part(&mut fingerprint, column);
        push_fingerprint_part(&mut fingerprint, &value_fingerprint(value));
    }
    fingerprint
}

fn push_fingerprint_part(fingerprint: &mut String, part: &str) {
    fingerprint.push_str(&part.len().to_string());
    fingerprint.push(':');
    fingerprint.push_str(part);
    fingerprint.push(';');
}

fn value_fingerprint(value: &RowValue) -> String {
    match value {
        RowValue::Null => "null".into(),
        RowValue::Bool(value) => format!("bool:{value}"),
        RowValue::I64(value) => format!("i64:{value}"),
        RowValue::U64(value) => format!("u64:{value}"),
        RowValue::F64(value) => format!("f64:{value:?}"),
        RowValue::String(value) => format!("string:{value}"),
        RowValue::Bytes(value) => format!("bytes:{value:?}"),
        RowValue::Json(value) => format!(
            "json:{}",
            serde_json::to_string(value).unwrap_or_else(|_| value.to_string())
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_fingerprint_distinguishes_delimiter_collisions() {
        let left = RowKey::new([
            ("a", RowValue::String("x,b=y".into())),
            ("b", RowValue::String("z".into())),
        ]);
        let right = RowKey::new([
            ("a", RowValue::String("x".into())),
            ("b", RowValue::String("y,b=z".into())),
        ]);

        assert_ne!(key_fingerprint(&left), key_fingerprint(&right));
    }

    #[test]
    fn key_fingerprint_distinguishes_row_value_types() {
        let integer = RowKey::new([("id", RowValue::I64(1))]);
        let string = RowKey::new([("id", RowValue::String("1".into()))]);

        assert_ne!(key_fingerprint(&integer), key_fingerprint(&string));
    }
}
