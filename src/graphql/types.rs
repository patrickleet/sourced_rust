//! Command-mutation GraphQL type metadata (input/output derives).

use std::any::TypeId;

use crate::graphql::naming::scalar_type_name;
use crate::read_model::RelationalReadModel;

/// One field on a GraphQL input or output object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphqlTypeField {
    pub name: String,
    pub type_name: String,
    pub nullable: bool,
    pub list: bool,
    /// Whether list elements are nullable. Always `false` for non-list fields.
    pub item_nullable: bool,
    /// Nested object type definition when `type_name` is not a scalar.
    pub nested: Option<Box<GraphqlTypeDef>>,
}

/// Full type definition for a derive-emitted input or output object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphqlTypeDef {
    pub name: String,
    pub fields: Vec<GraphqlTypeField>,
    pub type_id: Option<TypeId>,
}

impl GraphqlTypeDef {
    pub fn new(name: impl Into<String>, fields: Vec<GraphqlTypeField>) -> Self {
        Self {
            name: name.into(),
            fields,
            type_id: None,
        }
    }

    pub fn with_type_id(mut self, id: TypeId) -> Self {
        self.type_id = Some(id);
        self
    }

    /// Transitive nested type defs (depth-first, deduped by name).
    pub fn transitive_nested(&self) -> Vec<GraphqlTypeDef> {
        let mut out = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        self.collect_nested(&mut out, &mut seen);
        out
    }

    fn collect_nested(
        &self,
        out: &mut Vec<GraphqlTypeDef>,
        seen: &mut std::collections::BTreeSet<String>,
    ) {
        for field in &self.fields {
            if let Some(nested) = &field.nested {
                if seen.insert(nested.name.clone()) {
                    out.push((**nested).clone());
                    nested.collect_nested(out, seen);
                }
            }
        }
    }
}

pub trait GraphqlInputType {
    fn graphql_type() -> GraphqlTypeDef;
}

pub trait GraphqlOutputType {
    fn graphql_type() -> GraphqlTypeDef;
}

/// Build a command-output object from the same relational schema that owns
/// the generated query object.
///
/// Projected command results contain stored columns only. Relationships stay
/// query-time fields and are never invented by a same-transaction row result.
pub(crate) fn read_model_graphql_type<M>() -> GraphqlTypeDef
where
    M: RelationalReadModel + 'static,
{
    let schema = M::schema();
    let fields = schema
        .columns
        .iter()
        .filter(|column| !column.skipped)
        .map(|column| {
            let type_name = scalar_type_name(&column.column_type).unwrap_or_else(|| {
                panic!(
                    "read model `{}` column `{}` has no GraphQL scalar mapping",
                    schema.model_name, column.column_name
                )
            });
            GraphqlTypeField {
                name: column.column_name.clone(),
                type_name: type_name.into(),
                nullable: column.nullable,
                list: false,
                item_nullable: false,
                nested: None,
            }
        })
        .collect();
    GraphqlTypeDef::new(schema.model_name.clone(), fields).with_type_id(TypeId::of::<M>())
}

// Builtin scalar mappings for free-standing helpers used by derives.
#[allow(dead_code)]
pub fn scalar_for_rust_type(ty: &str) -> Option<&'static str> {
    match ty {
        "String" | "str" => Some("String"),
        "bool" => Some("Boolean"),
        "i8" | "i16" | "i32" | "i64" | "u8" | "u16" | "u32" | "u64" | "isize" | "usize" => {
            Some("BigInt")
        }
        "f32" | "f64" => Some("Float"),
        "Value" | "serde_json::Value" => Some("JSON"),
        _ => None,
    }
}
