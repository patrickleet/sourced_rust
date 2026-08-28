use super::*;

pub(in crate::graphql::surface) fn root_fields_for_models(
    models: &BTreeMap<String, SurfaceModel>,
    aggregates: bool,
    subscriptions: bool,
    default_limit: u64,
    max_limit: u64,
) -> (Vec<RootField>, Vec<RootField>) {
    let mut query_fields = Vec::new();
    let mut subscription_fields = Vec::new();
    for model in models.values() {
        let list = root_list_field(&model.schema).to_string();
        let by_pk = by_pk_field(&model.schema);
        query_fields.push(root_field(
            model,
            list.clone(),
            RootKind::List,
            default_limit,
            max_limit,
        ));
        query_fields.push(root_field(
            model,
            by_pk.clone(),
            RootKind::ByPk,
            default_limit,
            max_limit,
        ));
        if aggregates {
            query_fields.push(root_field(
                model,
                format!("{}_aggregate", model.table_name),
                RootKind::Aggregate,
                default_limit,
                max_limit,
            ));
        }
        if subscriptions {
            subscription_fields.push(root_field(
                model,
                list,
                RootKind::List,
                default_limit,
                max_limit,
            ));
        }
    }
    query_fields.sort_by(|a, b| a.name.cmp(&b.name));
    subscription_fields.sort_by(|a, b| a.name.cmp(&b.name));
    (query_fields, subscription_fields)
}

pub(in crate::graphql::surface) fn root_field(
    model: &SurfaceModel,
    name: String,
    kind: RootKind,
    default_limit: u64,
    max_limit: u64,
) -> RootField {
    let arguments = match kind {
        RootKind::List => list_arguments(&model.schema),
        RootKind::ByPk => primary_key_arguments(&model.schema),
        RootKind::Aggregate => vec![SurfaceArgument {
            name: "where".into(),
            kind: SurfaceArgumentKind::Filter,
            type_name: format!("{}_bool_exp", model.table_name),
            nullable: true,
            list: false,
        }],
    };
    let is_windowed = matches!(kind, RootKind::List);
    let effective_max = model.role_limit.unwrap_or(max_limit).min(max_limit);
    let role_default = default_limit.min(effective_max);
    RootField {
        name,
        kind,
        object: model.object_name.clone(),
        model_name: model.model_name.clone(),
        arguments,
        dependencies: vec![model.table_name.clone()],
        default_limit: is_windowed.then_some(role_default),
        max_limit: is_windowed.then_some(effective_max),
    }
}

pub(in crate::graphql::surface) fn list_arguments(schema: &TableSchema) -> Vec<SurfaceArgument> {
    vec![
        SurfaceArgument {
            name: "where".into(),
            kind: SurfaceArgumentKind::Filter,
            type_name: format!("{}_bool_exp", schema.table_name),
            nullable: true,
            list: false,
        },
        SurfaceArgument {
            name: "order_by".into(),
            kind: SurfaceArgumentKind::Order,
            type_name: format!("{}_order_by", schema.table_name),
            nullable: true,
            list: true,
        },
        SurfaceArgument {
            name: "limit".into(),
            kind: SurfaceArgumentKind::Limit,
            type_name: "Int".into(),
            nullable: true,
            list: false,
        },
        SurfaceArgument {
            name: "offset".into(),
            kind: SurfaceArgumentKind::Offset,
            type_name: "Int".into(),
            nullable: true,
            list: false,
        },
    ]
}

pub(in crate::graphql::surface) fn primary_key_arguments(
    schema: &TableSchema,
) -> Vec<SurfaceArgument> {
    schema
        .primary_key
        .columns
        .iter()
        .filter_map(|name| {
            let column = schema
                .columns
                .iter()
                .find(|column| column.column_name == *name && !column.skipped)?;
            let scalar = scalar_type_name(&column.column_type)?;
            Some(SurfaceArgument {
                name: name.clone(),
                kind: SurfaceArgumentKind::PrimaryKey,
                type_name: scalar.into(),
                nullable: false,
                list: false,
            })
        })
        .collect()
}

pub(in crate::graphql::surface) fn relationship_keys(
    source: &TableSchema,
    relationship: &crate::table::RelationshipDef,
    target: &TableSchema,
    by_table: &BTreeMap<&str, &TableSchema>,
) -> Result<(SurfaceRelationshipKeys, Vec<String>), String> {
    let mut dependencies = vec![source.table_name.clone(), target.table_name.clone()];
    let keys = match relationship.kind {
        RelationshipKind::BelongsTo => {
            let pairs = resolve_direct_join_keys(source, relationship, target)
                .map_err(|error| error.to_string())?;
            SurfaceRelationshipKeys::Direct {
                local: pairs
                    .iter()
                    .map(|pair| pair.foreign_key_column.clone())
                    .collect(),
                remote: pairs
                    .into_iter()
                    .map(|pair| pair.primary_key_column)
                    .collect(),
            }
        }
        RelationshipKind::HasMany => {
            let pairs = resolve_direct_join_keys(source, relationship, target)
                .map_err(|error| error.to_string())?;
            SurfaceRelationshipKeys::Direct {
                local: pairs
                    .iter()
                    .map(|pair| pair.primary_key_column.clone())
                    .collect(),
                remote: pairs
                    .into_iter()
                    .map(|pair| pair.foreign_key_column)
                    .collect(),
            }
        }
        RelationshipKind::ManyToMany => {
            let through_name = relationship.through.as_deref().ok_or_else(|| {
                format!(
                    "model `{}` relationship `{}` is missing through table",
                    source.model_name, relationship.field_name
                )
            })?;
            let through = by_table.get(through_name).ok_or_else(|| {
                format!(
                    "model `{}` relationship `{}` references missing through table `{through_name}`",
                    source.model_name, relationship.field_name
                )
            })?;
            let join_keys = resolve_m2m_join_keys(source, relationship, through, target)
                .map_err(|error| error.to_string())?;
            dependencies.push(through.table_name.clone());
            SurfaceRelationshipKeys::Through {
                local: source.primary_key.columns.clone(),
                remote: target.primary_key.columns.clone(),
                table: through.table_name.clone(),
                source_foreign_key: join_keys
                    .parent
                    .into_iter()
                    .map(|pair| pair.through_column)
                    .collect(),
                target_foreign_key: join_keys
                    .target
                    .into_iter()
                    .map(|pair| pair.through_column)
                    .collect(),
            }
        }
    };
    Ok((keys, dependencies))
}

pub(in crate::graphql::surface) fn visible_columns(
    schema: &TableSchema,
) -> impl Iterator<Item = &TableColumn> {
    schema.columns.iter().filter(|c| !c.skipped)
}

pub(in crate::graphql::surface) fn relationship_emitted(
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
            if let Some(through) = by_table.get(through_name) {
                resolve_m2m_join_keys(schema, rel, through, target).is_ok()
            } else {
                false
            }
        }
    }
}
