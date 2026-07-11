//! GraphQL name-derivation rules shared by the dep-free SDL renderer and the
//! dynamic schema builder. Every generated name lives here and only here.

use crate::table::{ColumnType, TableSchema};

/// GraphQL name grammar: `[_A-Za-z][_0-9A-Za-z]*` with no leading `__`.
pub fn is_valid_graphql_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some('_') => {
            if name.starts_with("__") {
                return false;
            }
            chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        }
        Some(c) if c.is_ascii_alphabetic() => {
            chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        }
        _ => false,
    }
}

pub fn object_type_name(schema: &TableSchema) -> &str {
    &schema.model_name
}

pub fn root_list_field(schema: &TableSchema) -> &str {
    &schema.table_name
}

pub fn by_pk_field(schema: &TableSchema) -> String {
    format!("{}_by_pk", schema.table_name)
}

pub fn aggregate_field(schema: &TableSchema) -> String {
    format!("{}_aggregate", schema.table_name)
}

pub fn bool_exp_name(schema: &TableSchema) -> String {
    format!("{}_bool_exp", schema.table_name)
}

pub fn order_by_name(schema: &TableSchema) -> String {
    format!("{}_order_by", schema.table_name)
}

pub fn aggregate_type_name(schema: &TableSchema) -> String {
    format!("{}_aggregate", schema.table_name)
}

pub fn aggregate_fields_type_name(schema: &TableSchema) -> String {
    format!("{}_aggregate_fields", schema.table_name)
}

pub fn sum_fields_type_name(schema: &TableSchema) -> String {
    format!("{}_sum_fields", schema.table_name)
}

pub fn avg_fields_type_name(schema: &TableSchema) -> String {
    format!("{}_avg_fields", schema.table_name)
}

pub fn min_fields_type_name(schema: &TableSchema) -> String {
    format!("{}_min_fields", schema.table_name)
}

pub fn max_fields_type_name(schema: &TableSchema) -> String {
    format!("{}_max_fields", schema.table_name)
}

/// Map a column type to its GraphQL scalar type name (without nullability).
pub fn scalar_type_name(column_type: &ColumnType) -> Option<&'static str> {
    match column_type {
        ColumnType::Text => Some("String"),
        ColumnType::Boolean => Some("Boolean"),
        ColumnType::Integer | ColumnType::UnsignedInteger => Some("BigInt"),
        ColumnType::Float => Some("Float"),
        ColumnType::Json => Some("JSON"),
        ColumnType::Timestamp => Some("Timestamptz"),
        ColumnType::Bytes => Some("Bytea"),
        ColumnType::Unsupported(_) => None,
    }
}

pub fn comparison_exp_name(scalar: &str) -> String {
    format!("{scalar}_comparison_exp")
}

/// Custom scalars emitted once, alphabetically.
pub const CUSTOM_SCALARS: &[&str] = &["BigInt", "Bytea", "JSON", "Timestamptz"];

/// Built-in + custom scalars that are reserved and must not collide with
/// generated type names.
pub fn reserved_type_names() -> impl Iterator<Item = &'static str> {
    [
        "String",
        "Boolean",
        "Int",
        "Float",
        "ID",
        "Query",
        "Mutation",
        "Subscription",
        "order_by",
        "BigInt",
        "Bytea",
        "JSON",
        "Timestamptz",
    ]
    .into_iter()
}

pub fn order_by_enum_values() -> &'static [&'static str] {
    &[
        "asc",
        "asc_nulls_first",
        "asc_nulls_last",
        "desc",
        "desc_nulls_first",
        "desc_nulls_last",
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_graphql_names() {
        assert!(is_valid_graphql_name("players"));
        assert!(is_valid_graphql_name("_private"));
        assert!(is_valid_graphql_name("PlayerView"));
        assert!(!is_valid_graphql_name("__typename"));
        assert!(!is_valid_graphql_name("1players"));
        assert!(!is_valid_graphql_name("play-ers"));
        assert!(!is_valid_graphql_name(""));
    }
}
