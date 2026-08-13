use super::*;

pub(in crate::graphql::surface) fn validate_nonempty_unique_ids(
    values: &[String],
    label: &str,
) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for value in values {
        if value.trim().is_empty() {
            return Err(format!("{label} id must not be empty"));
        }
        if !seen.insert(value) {
            return Err(format!("duplicate {label} id `{value}`"));
        }
    }
    Ok(())
}

pub(in crate::graphql::surface) fn validate_root_ids(
    fields: &[RootField],
    operation: &str,
) -> Result<(), String> {
    let mut names = BTreeSet::new();
    for field in fields {
        if field.name.trim().is_empty() {
            return Err(format!("{operation} root id must not be empty"));
        }
        if !names.insert(&field.name) {
            return Err(format!(
                "duplicate {operation} root id `{}` in Surface inventory",
                field.name
            ));
        }
    }
    Ok(())
}

pub(in crate::graphql::surface) fn validate_generated_surface_names(
    models: &BTreeMap<String, SurfaceModel>,
    comparison_ops: &BTreeMap<String, Vec<String>>,
) -> Result<(), String> {
    let mut type_names: BTreeSet<String> = reserved_type_names().map(str::to_string).collect();
    let mut claim_type = |name: String| -> Result<(), String> {
        if !is_valid_graphql_name(&name) {
            return Err(format!(
                "generated type name `{name}` is not a valid GraphQL name"
            ));
        }
        if !type_names.insert(name.clone()) {
            return Err(format!(
                "generated type name `{name}` collides with another Surface type"
            ));
        }
        Ok(())
    };
    for name in comparison_ops.keys() {
        claim_type(name.clone())?;
    }
    for model in models.values() {
        claim_type(model.object_name.clone())?;
        claim_type(format!("{}_bool_exp", model.table_name))?;
        claim_type(format!("{}_order_by", model.table_name))?;
        if model.aggregations {
            claim_type(format!("{}_aggregate", model.table_name))?;
            claim_type(format!("{}_aggregate_fields", model.table_name))?;
        }

        let mut object_fields = BTreeSet::new();
        for column in &model.columns {
            if matches!(column.name.as_str(), "_and" | "_or" | "_not") {
                return Err(format!(
                    "model `{}` field `{}` collides with a generated boolean-expression field",
                    model.model_name, column.name
                ));
            }
            if !object_fields.insert(column.name.clone()) {
                return Err(format!(
                    "model `{}` has duplicate GraphQL object field `{}`",
                    model.model_name, column.name
                ));
            }
        }
        for relationship in &model.relationships {
            if matches!(relationship.name.as_str(), "_and" | "_or" | "_not") {
                return Err(format!(
                    "model `{}` relationship `{}` collides with a generated boolean-expression field",
                    model.model_name, relationship.name
                ));
            }
            if !object_fields.insert(relationship.name.clone()) {
                return Err(format!(
                    "model `{}` relationship `{}` collides with another object field",
                    model.model_name, relationship.name
                ));
            }
            if let Some(aggregate) = &relationship.aggregate {
                if !object_fields.insert(aggregate.name.clone()) {
                    return Err(format!(
                        "model `{}` relationship aggregate `{}` collides with another object field",
                        model.model_name, aggregate.name
                    ));
                }
            }
        }
    }
    Ok(())
}
