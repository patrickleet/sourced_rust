//! Dep-free SDL text renderer for `dctl schema --format graphql`.
//!
//! Renders the dialect-independent core query surface from `&[TableSchema]`.
//! Artifact scope grows with the crate version (aggregates in phase 3,
//! Subscription root in phase 4). Renderer and engine ship together.

use std::collections::{BTreeMap, BTreeSet};

use crate::table::{
    resolve_m2m_target_foreign_key, ColumnType, RelationshipKind, TableKind, TableSchema,
};

use super::naming::{
    aggregate_field, aggregate_fields_type_name, aggregate_type_name, avg_fields_type_name,
    bool_exp_name, by_pk_field, comparison_exp_name, include_postgres_json_comparison_ops,
    is_valid_graphql_name, max_fields_type_name, min_fields_type_name, object_type_name,
    order_by_enum_values, order_by_name, reserved_type_names, root_list_field, scalar_type_name,
    sum_fields_type_name, CUSTOM_SCALARS, POSTGRES_JSON_COMPARISON_OPS,
};

/// Options controlling which surface slices the renderer emits.
#[derive(Clone, Debug)]
pub struct SdlOptions {
    /// Emit `<table>_aggregate` roots and nested aggregate fields (phase 3).
    pub aggregates: bool,
    /// Emit Postgres `jsonb` comparison operators on `JSON_comparison_exp`.
    ///
    /// Must match the runtime engine dialect: **false for SQLite**, true for
    /// Postgres. Defaults to false (SQLite / dialect-independent artifact).
    /// See [`SdlOptions::sqlite`] / [`SdlOptions::postgres`].
    pub jsonb_operators: bool,
    /// Emit a Subscription root mirroring Query list/by_pk fields (phase 4).
    pub subscriptions: bool,
}

impl Default for SdlOptions {
    fn default() -> Self {
        Self::sqlite()
    }
}

impl SdlOptions {
    /// SDL for SQLite-backed engines (no PG JSON comparison ops).
    pub fn sqlite() -> Self {
        Self {
            aggregates: true,
            jsonb_operators: include_postgres_json_comparison_ops(false),
            subscriptions: true,
        }
    }

    /// SDL for Postgres-backed engines (includes jsonb comparison ops).
    pub fn postgres() -> Self {
        Self {
            aggregates: true,
            jsonb_operators: include_postgres_json_comparison_ops(true),
            subscriptions: true,
        }
    }
}

/// Render GraphQL SDL for the given tables (ReadModel only; operational filtered).
pub fn graphql_sdl_for_tables(tables: &[TableSchema]) -> Result<String, String> {
    graphql_sdl_for_tables_with_options(tables, &SdlOptions::default())
}

pub fn graphql_sdl_for_tables_with_options(
    tables: &[TableSchema],
    options: &SdlOptions,
) -> Result<String, String> {
    let read_models: Vec<&TableSchema> = tables
        .iter()
        .filter(|t| t.kind.is_read_model())
        .collect();

    let by_model: BTreeMap<&str, &TableSchema> = read_models
        .iter()
        .map(|t| (t.model_name.as_str(), *t))
        .collect();
    let by_table: BTreeMap<&str, &TableSchema> = read_models
        .iter()
        .map(|t| (t.table_name.as_str(), *t))
        .collect();

    // Validate every table first.
    for schema in &read_models {
        schema
            .validate()
            .map_err(|e| format!("schema `{}` invalid: {e}", schema.model_name))?;
    }

    // Type names and root field names are separate GraphQL namespaces (Hasura
    // reuses e.g. `players_aggregate` as both a root field and an object type).
    let mut type_names: BTreeSet<String> = BTreeSet::new();
    let mut root_fields: BTreeSet<String> = BTreeSet::new();
    for reserved in reserved_type_names() {
        type_names.insert(reserved.to_string());
    }
    for scalar in CUSTOM_SCALARS {
        if !is_valid_graphql_name(scalar) {
            return Err(format!("scalar `{scalar}` is not a valid GraphQL name"));
        }
    }

    for schema in &read_models {
        claim_name(&mut type_names, object_type_name(schema))?;
        claim_name(&mut root_fields, root_list_field(schema))?;
        claim_name(&mut root_fields, &by_pk_field(schema))?;
        claim_name(&mut type_names, &bool_exp_name(schema))?;
        claim_name(&mut type_names, &order_by_name(schema))?;
        if options.aggregates {
            claim_name(&mut root_fields, &aggregate_field(schema))?;
            claim_name(&mut type_names, &aggregate_type_name(schema))?;
            claim_name(&mut type_names, &aggregate_fields_type_name(schema))?;
            claim_name(&mut type_names, &sum_fields_type_name(schema))?;
            claim_name(&mut type_names, &avg_fields_type_name(schema))?;
            claim_name(&mut type_names, &min_fields_type_name(schema))?;
            claim_name(&mut type_names, &max_fields_type_name(schema))?;
        }
        for column in visible_columns(schema) {
            let Some(scalar) = scalar_type_name(&column.column_type) else {
                return Err(format!(
                    "model `{}` column `{}` has unsupported type",
                    schema.model_name, column.column_name
                ));
            };
            let cmp = comparison_exp_name(scalar);
            if !type_names.contains(&cmp) {
                claim_name(&mut type_names, &cmp)?;
            }
            if !is_valid_graphql_name(&column.column_name) {
                return Err(format!(
                    "model `{}` column `{}` is not a valid GraphQL name",
                    schema.model_name, column.column_name
                ));
            }
        }
        for rel in &schema.relationships {
            if !is_valid_graphql_name(&rel.field_name) {
                return Err(format!(
                    "model `{}` relationship `{}` is not a valid GraphQL name",
                    schema.model_name, rel.field_name
                ));
            }
            if matches!(rel.kind, RelationshipKind::ManyToMany) {
                if rel.through.is_none() {
                    return Err(format!(
                        "model `{}` relationship `{}` many-to-many must declare `through`",
                        schema.model_name, rel.field_name
                    ));
                }
                if let (Some(target), Some(through_name)) = (
                    by_model.get(rel.target_model.as_str()),
                    rel.through.as_deref(),
                ) {
                    if let Some(through) = by_table.get(through_name) {
                        resolve_m2m_target_foreign_key(schema, rel, through, target)
                            .map_err(|e| e.to_string())?;
                    }
                }
            }
        }
    }

    let mut out = String::new();

    // Custom scalars, alphabetically.
    for scalar in CUSTOM_SCALARS {
        out.push_str(&format!("scalar {scalar}\n"));
    }
    out.push('\n');

    // order_by enum.
    out.push_str("enum order_by {\n");
    for v in order_by_enum_values() {
        out.push_str(&format!("  {v}\n"));
    }
    out.push_str("}\n\n");

    // Comparison input types (shared per scalar that appears).
    let mut used_scalars: BTreeSet<&str> = BTreeSet::new();
    for schema in &read_models {
        for column in visible_columns(schema) {
            if let Some(s) = scalar_type_name(&column.column_type) {
                used_scalars.insert(s);
            }
        }
    }
    for scalar in &used_scalars {
        emit_comparison_exp(&mut out, scalar, options.jsonb_operators);
    }

    // Per-table types: alphabetical by model_name for type block ordering of
    // object types; inputs follow similarly.
    let mut sorted_models: Vec<&&TableSchema> = read_models.iter().collect();
    sorted_models.sort_by(|a, b| a.model_name.cmp(&b.model_name));

    for schema in &sorted_models {
        emit_object_type(&mut out, schema, &by_model, &by_table, options);
        emit_bool_exp(&mut out, schema, &by_model, &by_table);
        emit_order_by_input(&mut out, schema);
        if options.aggregates {
            emit_aggregate_types(&mut out, schema);
        }
    }

    // Query root — fields alphabetical.
    out.push_str("type Query {\n");
    let mut root_fields: Vec<String> = Vec::new();
    for schema in &sorted_models {
        let table = root_list_field(schema);
        let bool_exp = bool_exp_name(schema);
        let order_by = order_by_name(schema);
        let obj = object_type_name(schema);
        root_fields.push(format!(
            "  {table}(where: {bool_exp}, order_by: [{order_by}!], limit: Int, offset: Int): [{obj}!]!"
        ));
        let by_pk = by_pk_field(schema);
        let pk_args = schema
            .primary_key
            .columns
            .iter()
            .filter_map(|pk| {
                let col = schema.columns.iter().find(|c| c.column_name == *pk)?;
                let scalar = scalar_type_name(&col.column_type)?;
                Some(format!("{pk}: {scalar}!"))
            })
            .collect::<Vec<_>>()
            .join(", ");
        root_fields.push(format!("  {by_pk}({pk_args}): {obj}"));
        if options.aggregates {
            let agg = aggregate_field(schema);
            let agg_ty = aggregate_type_name(schema);
            root_fields.push(format!("  {agg}(where: {bool_exp}): {agg_ty}"));
        }
    }
    root_fields.sort();
    for f in &root_fields {
        out.push_str(f);
        out.push('\n');
    }
    out.push_str("}\n");

    if options.subscriptions {
        out.push_str("\ntype Subscription {\n");
        let mut sub_fields: Vec<String> = Vec::new();
        for schema in &sorted_models {
            let table = root_list_field(schema);
            let bool_exp = bool_exp_name(schema);
            let order_by = order_by_name(schema);
            let obj = object_type_name(schema);
            sub_fields.push(format!(
                "  {table}(where: {bool_exp}, order_by: [{order_by}!], limit: Int, offset: Int): [{obj}!]!"
            ));
            let by_pk = by_pk_field(schema);
            let pk_args = schema
                .primary_key
                .columns
                .iter()
                .filter_map(|pk| {
                    let col = schema.columns.iter().find(|c| c.column_name == *pk)?;
                    let scalar = scalar_type_name(&col.column_type)?;
                    Some(format!("{pk}: {scalar}!"))
                })
                .collect::<Vec<_>>()
                .join(", ");
            sub_fields.push(format!("  {by_pk}({pk_args}): {obj}"));
        }
        sub_fields.sort();
        for f in &sub_fields {
            out.push_str(f);
            out.push('\n');
        }
        out.push_str("}\n");
    }

    Ok(out)
}

fn claim_name(names: &mut BTreeSet<String>, name: &str) -> Result<(), String> {
    if !is_valid_graphql_name(name) {
        return Err(format!("generated name `{name}` is not a valid GraphQL name"));
    }
    if !names.insert(name.to_string()) {
        return Err(format!("generated name `{name}` collides with another type or field"));
    }
    Ok(())
}

fn visible_columns(schema: &TableSchema) -> impl Iterator<Item = &crate::table::TableColumn> {
    schema.columns.iter().filter(|c| !c.skipped)
}

fn emit_comparison_exp(out: &mut String, scalar: &str, jsonb_ops: bool) {
    let name = comparison_exp_name(scalar);
    out.push_str(&format!("input {name} {{\n"));
    out.push_str(&format!("  _eq: {scalar}\n"));
    out.push_str(&format!("  _neq: {scalar}\n"));
    out.push_str(&format!("  _gt: {scalar}\n"));
    out.push_str(&format!("  _gte: {scalar}\n"));
    out.push_str(&format!("  _lt: {scalar}\n"));
    out.push_str(&format!("  _lte: {scalar}\n"));
    out.push_str(&format!("  _in: [{scalar}!]\n"));
    out.push_str(&format!("  _nin: [{scalar}!]\n"));
    out.push_str("  _is_null: Boolean\n");
    if *scalar == *"String" {
        out.push_str("  _like: String\n");
        out.push_str("  _ilike: String\n");
    }
    // Keep field list in lockstep with runtime schema via shared constants.
    if *scalar == *"JSON" && jsonb_ops {
        debug_assert!(include_postgres_json_comparison_ops(true));
        for op in POSTGRES_JSON_COMPARISON_OPS {
            match *op {
                "_contains" | "_contained_in" => {
                    out.push_str(&format!("  {op}: JSON\n"));
                }
                "_has_key" => out.push_str("  _has_key: String\n"),
                other => out.push_str(&format!("  {other}: JSON\n")),
            }
        }
    }
    out.push_str("}\n\n");
}

fn relationship_emitted(
    schema: &TableSchema,
    rel: &crate::table::RelationshipDef,
    by_model: &BTreeMap<&str, &TableSchema>,
    by_table: &BTreeMap<&str, &TableSchema>,
) -> bool {
    let Some(target) = by_model.get(rel.target_model.as_str()) else {
        return false;
    };
    match rel.kind {
        RelationshipKind::HasMany | RelationshipKind::BelongsTo => true,
        RelationshipKind::ManyToMany => {
            let Some(through_name) = rel.through.as_deref() else {
                return false;
            };
            if by_table.get(through_name).is_none() {
                return false;
            }
            // Inference must succeed for emission.
            if let Some(through) = by_table.get(through_name) {
                resolve_m2m_target_foreign_key(schema, rel, through, target).is_ok()
            } else {
                false
            }
        }
    }
}

fn emit_object_type(
    out: &mut String,
    schema: &TableSchema,
    by_model: &BTreeMap<&str, &TableSchema>,
    by_table: &BTreeMap<&str, &TableSchema>,
    options: &SdlOptions,
) {
    let name = object_type_name(schema);
    out.push_str(&format!("type {name} {{\n"));
    for column in visible_columns(schema) {
        let Some(scalar) = scalar_type_name(&column.column_type) else {
            continue;
        };
        let null = if column.nullable { "" } else { "!" };
        out.push_str(&format!("  {}: {}{}\n", column.column_name, scalar, null));
    }
    for rel in &schema.relationships {
        if !relationship_emitted(schema, rel, by_model, by_table) {
            continue;
        }
        let target = by_model
            .get(rel.target_model.as_str())
            .expect("checked in relationship_emitted");
        let target_obj = object_type_name(target);
        match rel.kind {
            RelationshipKind::BelongsTo => {
                let fk_nullable = schema
                    .columns
                    .iter()
                    .find(|c| {
                        c.column_name == rel.foreign_key.as_deref().unwrap_or("")
                            || c.field_name == rel.foreign_key.as_deref().unwrap_or("")
                    })
                    .map(|c| c.nullable)
                    .unwrap_or(true);
                let null = if fk_nullable { "" } else { "!" };
                out.push_str(&format!("  {}: {}{}\n", rel.field_name, target_obj, null));
            }
            RelationshipKind::HasMany | RelationshipKind::ManyToMany => {
                let bool_exp = bool_exp_name(target);
                let order_by = order_by_name(target);
                out.push_str(&format!(
                    "  {}(where: {}, order_by: [{}!], limit: Int, offset: Int): [{}!]!\n",
                    rel.field_name, bool_exp, order_by, target_obj
                ));
                if options.aggregates {
                    let agg_ty = aggregate_type_name(target);
                    out.push_str(&format!(
                        "  {}_aggregate(where: {}): {}\n",
                        rel.field_name, bool_exp, agg_ty
                    ));
                }
            }
        }
    }
    out.push_str("}\n\n");
}

fn emit_bool_exp(
    out: &mut String,
    schema: &TableSchema,
    by_model: &BTreeMap<&str, &TableSchema>,
    by_table: &BTreeMap<&str, &TableSchema>,
) {
    let name = bool_exp_name(schema);
    out.push_str(&format!("input {name} {{\n"));
    out.push_str(&format!("  _and: [{name}!]\n"));
    out.push_str(&format!("  _or: [{name}!]\n"));
    out.push_str(&format!("  _not: {name}\n"));
    for column in visible_columns(schema) {
        let Some(scalar) = scalar_type_name(&column.column_type) else {
            continue;
        };
        let cmp = comparison_exp_name(scalar);
        out.push_str(&format!("  {}: {}\n", column.column_name, cmp));
    }
    for rel in &schema.relationships {
        if !relationship_emitted(schema, rel, by_model, by_table) {
            continue;
        }
        let target = by_model
            .get(rel.target_model.as_str())
            .expect("checked");
        let target_bool = bool_exp_name(target);
        out.push_str(&format!("  {}: {}\n", rel.field_name, target_bool));
    }
    out.push_str("}\n\n");
}

fn emit_order_by_input(out: &mut String, schema: &TableSchema) {
    let name = order_by_name(schema);
    out.push_str(&format!("input {name} {{\n"));
    for column in visible_columns(schema) {
        out.push_str(&format!("  {}: order_by\n", column.column_name));
    }
    out.push_str("}\n\n");
}

fn emit_aggregate_types(out: &mut String, schema: &TableSchema) {
    let agg = aggregate_type_name(schema);
    let fields = aggregate_fields_type_name(schema);
    let obj = object_type_name(schema);
    out.push_str(&format!("type {agg} {{\n"));
    out.push_str(&format!("  aggregate: {fields}\n"));
    out.push_str(&format!("  nodes: [{obj}!]!\n"));
    out.push_str("}\n\n");

    out.push_str(&format!("type {fields} {{\n"));
    out.push_str("  count: Int!\n");
    let numeric: Vec<_> = visible_columns(schema)
        .filter(|c| {
            matches!(
                c.column_type,
                ColumnType::Integer | ColumnType::UnsignedInteger | ColumnType::Float
            )
        })
        .collect();
    if !numeric.is_empty() {
        out.push_str(&format!("  sum: {}\n", sum_fields_type_name(schema)));
        out.push_str(&format!("  avg: {}\n", avg_fields_type_name(schema)));
    }
    out.push_str(&format!("  min: {}\n", min_fields_type_name(schema)));
    out.push_str(&format!("  max: {}\n", max_fields_type_name(schema)));
    out.push_str("}\n\n");

    if !numeric.is_empty() {
        for (ty_name, as_float) in [
            (sum_fields_type_name(schema), false),
            (avg_fields_type_name(schema), true),
        ] {
            out.push_str(&format!("type {ty_name} {{\n"));
            for col in &numeric {
                let scalar = if as_float {
                    "Float"
                } else {
                    scalar_type_name(&col.column_type).unwrap_or("BigInt")
                };
                out.push_str(&format!("  {}: {}\n", col.column_name, scalar));
            }
            out.push_str("}\n\n");
        }
    }

    for ty_name in [min_fields_type_name(schema), max_fields_type_name(schema)] {
        out.push_str(&format!("type {ty_name} {{\n"));
        for col in visible_columns(schema) {
            if matches!(col.column_type, ColumnType::Json | ColumnType::Bytes) {
                continue;
            }
            let Some(scalar) = scalar_type_name(&col.column_type) else {
                continue;
            };
            out.push_str(&format!("  {}: {}\n", col.column_name, scalar));
        }
        out.push_str("}\n\n");
    }
}

/// Filter operational tables and render SDL for a project manifest's tables.
pub fn graphql_sdl_from_schemas(schemas: impl IntoIterator<Item = TableSchema>) -> Result<String, String> {
    let tables: Vec<TableSchema> = schemas
        .into_iter()
        .filter(|t| matches!(t.kind, TableKind::ReadModel))
        .collect();
    graphql_sdl_for_tables(&tables)
}
