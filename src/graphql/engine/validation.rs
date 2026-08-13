use super::*;

pub(crate) fn validate_generated_names(
    catalog: &BTreeMap<String, CatalogEntry>,
) -> Result<(), GraphqlBuildError> {
    let mut names: BTreeSet<String> = reserved_type_names().map(str::to_string).collect();
    for entry in catalog.values().filter(|e| e.exposed) {
        let schema = &entry.schema;
        for name in [
            object_type_name(schema).to_string(),
            root_list_field(schema).to_string(),
            by_pk_field(schema),
            format!("{}_bool_exp", schema.table_name),
            format!("{}_order_by", schema.table_name),
            format!("{}_aggregate", schema.table_name),
        ] {
            if !is_valid_graphql_name(&name) {
                return Err(GraphqlBuildError(format!(
                    "generated name `{name}` is not a valid GraphQL name"
                )));
            }
            if !names.insert(name.clone()) {
                return Err(GraphqlBuildError(format!(
                    "generated name `{name}` collides with another type or field"
                )));
            }
        }
    }
    Ok(())
}

pub(crate) fn validate_filter(
    filter: &FilterExpr,
    schema: &TableSchema,
    catalog: &BTreeMap<String, CatalogEntry>,
    is_anonymous: bool,
    model: &str,
    role: &str,
) -> Result<(), GraphqlBuildError> {
    filter.validate_row_policy_literals().map_err(|error| {
        GraphqlBuildError(format!(
            "invalid row policy for model `{model}` role `{role}`: {error}"
        ))
    })?;
    if is_anonymous {
        let mut claims = Vec::new();
        filter.visit_claims(|c| claims.push(c.to_string()));
        if !claims.is_empty() {
            return Err(GraphqlBuildError(format!(
                "claim() is not allowed in anonymous role filters (model `{model}`, claims: {})",
                claims.join(", ")
            )));
        }
    }

    filter.visit_columns(|col| {
        let _ = col;
    });
    // Re-walk for proper error returns.
    validate_filter_inner(filter, schema, catalog, model, role)
}

fn validate_filter_inner(
    filter: &FilterExpr,
    schema: &TableSchema,
    catalog: &BTreeMap<String, CatalogEntry>,
    model: &str,
    role: &str,
) -> Result<(), GraphqlBuildError> {
    match filter {
        FilterExpr::And(xs) | FilterExpr::Or(xs) => {
            for x in xs {
                validate_filter_inner(x, schema, catalog, model, role)?;
            }
        }
        FilterExpr::Not(x) => validate_filter_inner(x, schema, catalog, model, role)?,
        FilterExpr::Cmp { column, op, rhs } => {
            let col = schema
                .columns
                .iter()
                .find(|c| c.column_name == *column)
                .ok_or_else(|| {
                    GraphqlBuildError(format!(
                        "unknown column `{column}` in filter for `{model}` role `{role}`"
                    ))
                })?;
            if matches!(col.column_type, ColumnType::Json) && matches!(rhs, Operand::Claim(_)) {
                return Err(GraphqlBuildError(format!(
                    "claims cannot compare to Json columns (`{column}` on `{model}`)"
                )));
            }
            validate_row_policy_operand_literal(column, &col.column_type, Some(*op), rhs).map_err(
                |error| {
                    GraphqlBuildError(format!(
                        "invalid row policy for model `{model}` role `{role}`: {error}"
                    ))
                },
            )?;
        }
        FilterExpr::In { column, values, .. } => {
            let col = schema
                .columns
                .iter()
                .find(|candidate| candidate.column_name == *column)
                .ok_or_else(|| {
                    GraphqlBuildError(format!(
                        "unknown column `{column}` in filter for `{model}` role `{role}`"
                    ))
                })?;
            for (index, value) in values.iter().enumerate() {
                validate_row_policy_operand_literal(column, &col.column_type, None, value)
                    .map_err(|error| {
                        GraphqlBuildError(format!(
                            "invalid row policy for model `{model}` role `{role}` IN operand {index}: {error}"
                        ))
                    })?;
            }
        }
        FilterExpr::IsNull { column, .. } => {
            if !schema.columns.iter().any(|c| c.column_name == *column) {
                return Err(GraphqlBuildError(format!(
                    "unknown column `{column}` in filter for `{model}` role `{role}`"
                )));
            }
        }
        FilterExpr::Rel { field, predicate } => {
            let rel = schema
                .relationships
                .iter()
                .find(|r| r.field_name == *field)
                .ok_or_else(|| {
                    GraphqlBuildError(format!(
                        "rel(`{field}`) is not a relationship on model `{model}`"
                    ))
                })?;
            let target = catalog.get(&rel.target_model).ok_or_else(|| {
                GraphqlBuildError(format!(
                    "rel(`{field}`) target `{}` is not in the catalog (model `{model}`)",
                    rel.target_model
                ))
            })?;
            if matches!(rel.kind, RelationshipKind::ManyToMany) {
                let through = rel.through.as_deref().ok_or_else(|| {
                    GraphqlBuildError(format!(
                        "rel(`{field}`) many-to-many missing through on `{model}`"
                    ))
                })?;
                let through_model = catalog
                    .values()
                    .find(|e| e.schema.table_name == through)
                    .ok_or_else(|| {
                        GraphqlBuildError(format!(
                            "rel(`{field}`) through table `{through}` not in catalog"
                        ))
                    })?;
                let _ = through_model;
            }
            validate_filter_inner(predicate, &target.schema, catalog, &rel.target_model, role)?;
        }
    }
    Ok(())
}

/// Execute a compiled plan against the engine pool (used by root resolvers).
pub(crate) async fn execute_plan(inner: &EngineInner, plan: &SqlPlan) -> Result<Value, String> {
    execute::execute_sql(inner, plan).await
}

/// Public helper for tests: compile + naming surface.
pub fn core_sdl_for_catalog(tables: &[TableSchema]) -> Result<String, String> {
    // Dialect-independent / SQLite-default SDL (no PG JSON ops).
    graphql_sdl_for_tables_with_options(tables, &SdlOptions::sqlite())
}
