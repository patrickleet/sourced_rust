use super::*;

/// Role grant used by [`surface_for_role`] (feature-free; maps from `ReadPermission`
/// when the `graphql` feature is enabled).
#[derive(Clone, Debug)]
pub struct RoleGrant {
    pub all_columns: bool,
    pub columns: BTreeSet<String>,
    pub aggregations: bool,
    pub row_policy: SurfaceRowPolicy,
    pub limit: Option<u64>,
}

impl RoleGrant {
    pub fn all_columns() -> Self {
        Self {
            all_columns: true,
            columns: BTreeSet::new(),
            aggregations: false,
            row_policy: SurfaceRowPolicy::Unrestricted,
            limit: None,
        }
    }

    pub fn columns<I: IntoIterator<Item = impl Into<String>>>(cols: I) -> Self {
        Self {
            all_columns: false,
            columns: cols.into_iter().map(Into::into).collect(),
            aggregations: false,
            row_policy: SurfaceRowPolicy::Unrestricted,
            limit: None,
        }
    }

    pub fn with_aggregations(mut self) -> Self {
        self.aggregations = true;
        self
    }

    pub fn rows(mut self, predicate: FilterExpr) -> Self {
        self.row_policy = SurfaceRowPolicy::Predicate(predicate);
        self
    }

    pub fn server_only_rows(mut self) -> Self {
        self.row_policy = SurfaceRowPolicy::ServerOnly;
        self
    }

    pub fn limit(mut self, limit: u64) -> Self {
        self.limit = Some(limit);
        self
    }

    pub fn allows_column(&self, name: &str) -> bool {
        self.all_columns || self.columns.contains(name)
    }
}

/// Build a role→grant map for one role from `(model_name, role) → grant` entries.
///
/// Entries whose role does not match are ignored. Used by export/SDL and engine
/// adapters that already store grants keyed by `(model, role)`.
pub fn role_grants_for_role(
    role: &str,
    model_role_grants: &BTreeMap<(String, String), RoleGrant>,
) -> BTreeMap<String, RoleGrant> {
    let mut out = BTreeMap::new();
    for ((model, r), grant) in model_role_grants {
        if r == role {
            out.insert(model.clone(), grant.clone());
        }
    }
    out
}

/// Apply role grants: drop ungranted models and columns (and relationships to
/// dropped models). Aggregate roots omitted when `aggregations` is false.
///
/// `grants`: map of model_name → grant for this role. Missing model = not granted.
/// Returns an error when a row policy contains a literal that cannot be
/// represented faithfully by the shared runtime/client contract.
pub fn surface_for_role(
    surface: &Surface,
    role: &str,
    grants: &BTreeMap<String, RoleGrant>,
) -> Result<Surface, String> {
    // Validate the complete declared topology before authorization filtering.
    // Only a projector hidden by a valid role selection may become
    // `confirmation_unavailable`; an omitted catalog topology is an error.
    validate_command_confirmation_topology(
        &surface.commands,
        &surface.projectors,
        &surface.models,
    )?;
    validate_role_grants(surface, role, grants)?;
    let mut models: BTreeMap<String, SurfaceModel> = BTreeMap::new();

    for (model_name, model) in &surface.models {
        let Some(grant) = grants.get(model_name) else {
            continue;
        };

        let allowed_cols: BTreeSet<String> = model
            .columns
            .iter()
            .filter(|c| grant.allows_column(&c.name))
            .map(|c| c.name.clone())
            .collect();

        let columns: Vec<ColumnField> = model
            .columns
            .iter()
            .filter(|c| allowed_cols.contains(&c.name))
            .cloned()
            .collect();

        let mut schema = model.schema.clone();
        for col in &mut schema.columns {
            if !col.skipped && !allowed_cols.contains(&col.column_name) {
                col.skipped = true;
            }
        }

        models.insert(
            model_name.clone(),
            SurfaceModel {
                model_name: model.model_name.clone(),
                table_name: model.table_name.clone(),
                object_name: model.object_name.clone(),
                columns,
                relationships: model.relationships.clone(),
                primary_key: model.primary_key.clone(),
                row_policy: grant.row_policy.clone(),
                role_limit: grant.limit,
                aggregations: grant.aggregations,
                schema,
            },
        );
    }

    // Relationships only if target model remains granted (collect keys first).
    let model_keys: BTreeSet<String> = models.keys().cloned().collect();
    for model in models.values_mut() {
        model
            .relationships
            .retain(|r| model_keys.contains(&r.target_model));
        let rel_names: BTreeSet<String> =
            model.relationships.iter().map(|r| r.name.clone()).collect();
        model
            .schema
            .relationships
            .retain(|r| model_keys.contains(&r.target_model) && rel_names.contains(&r.field_name));
    }

    validate_selected_composite_relationships(&models)?;
    sanitize_relationship_identity(&mut models);

    let aggregate_targets: BTreeMap<String, bool> = models
        .iter()
        .map(|(name, model)| (name.clone(), model.aggregations))
        .collect();
    for model in models.values_mut() {
        for relationship in &mut model.relationships {
            if !aggregate_targets
                .get(&relationship.target_model)
                .copied()
                .unwrap_or(false)
            {
                relationship.aggregate = None;
            } else if let Some(aggregate) = &mut relationship.aggregate {
                aggregate.dependencies = relationship.dependencies.clone();
            }
        }
    }

    // A row predicate is portable only when every referenced model/field is
    // present on this selected surface. Otherwise retain the authorization
    // fact as `ServerOnly` without leaking the hidden dependency.
    let model_names: Vec<String> = models.keys().cloned().collect();
    for model_name in model_names {
        let policy = models[&model_name].row_policy.clone();
        if let SurfaceRowPolicy::Predicate(predicate) = &policy {
            if !filter_is_surface_visible(predicate, &model_name, &models)
                || !predicate.is_client_portable()
            {
                models.get_mut(&model_name).expect("model key").row_policy =
                    SurfaceRowPolicy::ServerOnly;
            }
        }
    }

    let mut query_fields = Vec::new();
    let mut subscription_fields = Vec::new();
    for model in models.values() {
        let grant = grants.get(&model.model_name);
        let allow_agg = surface.aggregates && grant.is_some_and(|g| g.aggregations);
        let list = root_list_field(&model.schema).to_string();
        let by_pk = by_pk_field(&model.schema);
        query_fields.push(root_field(
            model,
            list.clone(),
            RootKind::List,
            surface.default_limit,
            surface.max_limit,
        ));
        let stable_key_visible = !model.primary_key.is_empty()
            && model
                .primary_key
                .iter()
                .all(|key| model.columns.iter().any(|column| column.name == *key));
        if stable_key_visible {
            query_fields.push(root_field(
                model,
                by_pk.clone(),
                RootKind::ByPk,
                surface.default_limit,
                surface.max_limit,
            ));
        }
        if allow_agg {
            query_fields.push(root_field(
                model,
                format!("{}_aggregate", model.table_name),
                RootKind::Aggregate,
                surface.default_limit,
                surface.max_limit,
            ));
        }
        if surface.subscriptions {
            subscription_fields.push(root_field(
                model,
                list,
                RootKind::List,
                surface.default_limit,
                surface.max_limit,
            ));
        }
    }
    query_fields.sort_by(|a, b| a.name.cmp(&b.name));
    subscription_fields.sort_by(|a, b| a.name.cmp(&b.name));

    let postgres_json = include_postgres_json_comparison_ops(surface.dialect.is_postgres());
    let mut used_scalars: BTreeSet<String> = BTreeSet::new();
    for m in models.values() {
        for c in &m.columns {
            used_scalars.insert(c.scalar.clone());
        }
    }
    let mut comparison_ops = BTreeMap::new();
    for scalar in &used_scalars {
        let ops = comparison_op_fields(scalar, postgres_json);
        comparison_ops.insert(
            comparison_exp_name(scalar),
            ops.into_iter().map(str::to_string).collect(),
        );
    }

    let aggregates = query_fields.iter().any(|f| f.kind == RootKind::Aggregate);

    let mut commands: Vec<SurfaceCommand> = surface
        .commands
        .iter()
        .filter(|command| {
            command.roles.is_empty() || command.roles.iter().any(|allowed| allowed == role)
        })
        .cloned()
        .collect();
    for command in &mut commands {
        command.roles = vec![role.to_string()];
    }
    sanitize_command_effects_for_models(&mut commands, &models);

    let mut projectors = Vec::new();
    for projector in &surface.projectors {
        // Facts do not carry per-model provenance. If any target is denied,
        // retaining a subset would leak fact IDs/topology from that denied
        // domain, so omit the whole projector.
        if projector
            .models
            .iter()
            .any(|model| !models.contains_key(model))
        {
            continue;
        }
        let dependencies = projector
            .models
            .iter()
            .filter_map(|model| models.get(model).map(|model| model.table_name.clone()))
            .collect();
        projectors.push(SurfaceProjectionOwner {
            name: projector.name.clone(),
            facts: projector.facts.clone(),
            models: projector.models.clone(),
            dependencies,
            change_epoch: projector.change_epoch.clone(),
            partition: projector.partition.clone(),
            kind: projector.kind,
            modeled: projector.modeled.clone(),
        });
    }
    sanitize_command_confirmations(&mut commands, &projectors, &models);

    Ok(Surface {
        selection: SurfaceSelection::Role {
            name: role.to_string(),
        },
        dialect: surface.dialect,
        aggregates,
        subscriptions: surface.subscriptions,
        default_limit: surface.default_limit,
        max_limit: surface.max_limit,
        catalog: surface.catalog.clone(),
        models,
        query_fields,
        subscription_fields,
        comparison_ops,
        commands,
        commands_attached: surface.commands_attached,
        projectors,
        projectors_attached: surface.projectors_attached,
        service_binding: surface.service_binding.clone(),
    })
}

/// Composite identities are valid for isolated roots. Relationship compilation
/// still uses a single-column join contract, so reject the topology only when
/// both ends are reachable on this selected authorization surface. Keeping the
/// check here makes runtime role schemas and pool-free/dctl exports fail at the
/// same boundary without rejecting unrelated hidden catalog metadata.
pub(in crate::graphql::surface) fn validate_selected_composite_relationships(
    models: &BTreeMap<String, SurfaceModel>,
) -> Result<(), String> {
    for model in models.values() {
        let pk_n = model.primary_key.len();
        if pk_n <= 1 {
            continue;
        }
        let relationship_topology = !model.relationships.is_empty()
            || models.values().any(|candidate| {
                candidate
                    .relationships
                    .iter()
                    .any(|relationship| relationship.target_model == model.model_name)
            });
        if relationship_topology {
            return Err(format!(
                "model `{}` has a {pk_n}-column primary key and relationship topology; composite-key GraphQL models are supported only as isolated roots until composite relationship keys are implemented",
                model.model_name
            ));
        }
    }
    Ok(())
}

pub(in crate::graphql::surface) fn validate_role_grants(
    surface: &Surface,
    role: &str,
    grants: &BTreeMap<String, RoleGrant>,
) -> Result<(), String> {
    for (model, grant) in grants {
        let selected_model = surface
            .models
            .get(model)
            .ok_or_else(|| format!("permission for unknown model `{model}` in surface `{role}`"))?;

        if !grant.all_columns {
            for column in &grant.columns {
                if !selected_model
                    .schema
                    .columns
                    .iter()
                    .any(|candidate| candidate.column_name == *column && !candidate.skipped)
                {
                    return Err(format!(
                        "unknown column `{column}` in permission for `{model}` surface `{role}`"
                    ));
                }
            }
        }

        if let SurfaceRowPolicy::Predicate(predicate) = &grant.row_policy {
            predicate.validate_row_policy_literals().map_err(|error| {
                format!("invalid row policy for model `{model}` in surface `{role}`: {error}")
            })?;
            validate_surface_filter(
                predicate,
                &selected_model.schema,
                &surface.catalog,
                model,
                role,
            )?;
        }
    }
    Ok(())
}

/// Validate the executable row-policy graph against the same complete catalog
/// used by the runtime compiler. This deliberately permits policy references
/// to denied columns/models (the policy remains server-enforced) while rejecting
/// identifiers and relationship shapes that the runtime cannot compile.
pub(in crate::graphql::surface) fn validate_surface_filter(
    filter: &FilterExpr,
    schema: &TableSchema,
    catalog: &BTreeMap<String, TableSchema>,
    model: &str,
    role: &str,
) -> Result<(), String> {
    match filter {
        FilterExpr::And(items) | FilterExpr::Or(items) => {
            for item in items {
                validate_surface_filter(item, schema, catalog, model, role)?;
            }
        }
        FilterExpr::Not(item) => {
            validate_surface_filter(item, schema, catalog, model, role)?;
        }
        FilterExpr::Cmp { column, op, rhs } => {
            let column_schema = schema
                .columns
                .iter()
                .find(|candidate| candidate.column_name == *column)
                .ok_or_else(|| {
                    format!("unknown column `{column}` in filter for `{model}` surface `{role}`")
                })?;
            if matches!(column_schema.column_type, ColumnType::Json)
                && matches!(rhs, Operand::Claim(_))
            {
                return Err(format!(
                    "claims cannot compare to Json columns (`{column}` on `{model}`)"
                ));
            }
            validate_row_policy_operand_literal(column, &column_schema.column_type, Some(*op), rhs)
                .map_err(|error| {
                    format!("invalid row policy for model `{model}` surface `{role}`: {error}")
                })?;
        }
        FilterExpr::In { column, values, .. } => {
            let column_schema = schema
                .columns
                .iter()
                .find(|candidate| candidate.column_name == *column)
                .ok_or_else(|| {
                    format!("unknown column `{column}` in filter for `{model}` surface `{role}`")
                })?;
            for (index, value) in values.iter().enumerate() {
                validate_row_policy_operand_literal(
                    column,
                    &column_schema.column_type,
                    None,
                    value,
                )
                .map_err(|error| {
                        format!(
                            "invalid row policy for model `{model}` surface `{role}` IN operand {index}: {error}"
                        )
                    })?;
            }
        }
        FilterExpr::IsNull { column, .. } => {
            if !schema
                .columns
                .iter()
                .any(|candidate| candidate.column_name == *column)
            {
                return Err(format!(
                    "unknown column `{column}` in filter for `{model}` surface `{role}`"
                ));
            }
        }
        FilterExpr::Rel { field, predicate } => {
            let relationship = schema
                .relationships
                .iter()
                .find(|candidate| candidate.field_name == *field)
                .ok_or_else(|| {
                    format!("rel(`{field}`) is not a relationship on model `{model}`")
                })?;
            let target = catalog.get(&relationship.target_model).ok_or_else(|| {
                format!(
                    "rel(`{field}`) target `{}` is not in the catalog (model `{model}`)",
                    relationship.target_model
                )
            })?;

            if schema.primary_key.columns.len() > 1 || target.primary_key.columns.len() > 1 {
                return Err(format!(
                    "row policy for model `{model}` surface `{role}` traverses relationship `{field}` with composite-key topology; composite relationship keys are not implemented"
                ));
            }

            if matches!(relationship.kind, RelationshipKind::ManyToMany) {
                let through = relationship.through.as_deref().ok_or_else(|| {
                    format!("rel(`{field}`) many-to-many missing through on `{model}`")
                })?;
                if !catalog
                    .values()
                    .any(|candidate| candidate.table_name == through)
                {
                    return Err(format!(
                        "rel(`{field}`) through table `{through}` not in catalog"
                    ));
                }
            }
            validate_surface_filter(predicate, target, catalog, &relationship.target_model, role)?;
        }
    }
    Ok(())
}

/// Build an explicit named application surface as the structural intersection
/// of all runtime roles it supports.
///
/// A missing role declaration is an error rather than an accidental empty or
/// admin surface. Commands must be granted to every role; differing row
/// predicates become `ServerOnly`, so the client revalidates membership without
/// learning another role's policy.
pub fn surface_for_application(
    surface: &Surface,
    application: &str,
    roles: &[String],
    grants_by_role: &BTreeMap<String, BTreeMap<String, RoleGrant>>,
) -> Result<Surface, String> {
    if application.trim().is_empty() {
        return Err("application surface name must not be empty".into());
    }
    let mut roles = roles.to_vec();
    roles.sort();
    roles.dedup();
    if roles.is_empty() {
        return Err(format!(
            "application surface `{application}` must declare at least one role"
        ));
    }
    for role in &roles {
        let Some(grants) = grants_by_role.get(role) else {
            return Err(format!(
                "application surface `{application}` references undeclared role `{role}`"
            ));
        };
        // Validate every concrete role before intersecting it. Differing
        // predicates collapse to ServerOnly below, but that must not hide a
        // malformed identifier or unsupported relationship traversal.
        let _ = surface_for_role(surface, role, grants)?;
    }

    let mut common = BTreeMap::new();
    for (model_name, model) in &surface.models {
        let grants: Option<Vec<&RoleGrant>> = roles
            .iter()
            .map(|role| {
                grants_by_role
                    .get(role)
                    .and_then(|grants| grants.get(model_name))
            })
            .collect();
        let Some(grants) = grants else {
            continue;
        };

        let columns: BTreeSet<String> = model
            .columns
            .iter()
            .map(|column| column.name.clone())
            .filter(|column| grants.iter().all(|grant| grant.allows_column(column)))
            .collect();
        let aggregations = grants.iter().all(|grant| grant.aggregations);
        let first_policy = grants[0].row_policy.clone();
        let row_policy = if grants.iter().all(|grant| grant.row_policy == first_policy) {
            first_policy
        } else {
            SurfaceRowPolicy::ServerOnly
        };
        let limit = grants.iter().filter_map(|grant| grant.limit).min();
        common.insert(
            model_name.clone(),
            RoleGrant {
                all_columns: false,
                columns,
                aggregations,
                row_policy,
                limit,
            },
        );
    }

    let mut selected = surface_for_role(surface, application, &common)?;
    selected.commands = surface
        .commands
        .iter()
        .filter(|command| {
            command.roles.is_empty()
                || roles
                    .iter()
                    .all(|role| command.roles.iter().any(|allowed| allowed == role))
        })
        .cloned()
        .map(|mut command| {
            command.roles = roles.clone();
            command
        })
        .collect();
    sanitize_command_effects_for_models(&mut selected.commands, &selected.models);
    sanitize_command_confirmations(
        &mut selected.commands,
        &selected.projectors,
        &selected.models,
    );
    selected
        .commands
        .sort_by(|a, b| a.command_name.cmp(&b.command_name));
    selected.selection = SurfaceSelection::Application {
        name: application.to_string(),
        roles,
    };
    Ok(selected)
}
