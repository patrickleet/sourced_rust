use super::*;

pub(in crate::graphql::surface) fn validate_and_canonicalize_commands(
    models: &BTreeMap<String, SurfaceModel>,
    comparison_ops: &BTreeMap<String, Vec<String>>,
    commands: &mut [SurfaceCommand],
) -> Result<(), String> {
    let mut names = BTreeSet::new();
    let mut fields = BTreeSet::new();
    let mut type_defs: BTreeMap<String, (bool, SurfaceTypeDef)> = BTreeMap::new();
    let mut occupied_types: BTreeSet<String> = reserved_type_names().map(str::to_string).collect();
    occupied_types.extend(comparison_ops.keys().cloned());
    for model in models.values() {
        occupied_types.insert(model.object_name.clone());
        occupied_types.insert(format!("{}_bool_exp", model.table_name));
        occupied_types.insert(format!("{}_order_by", model.table_name));
        if model.aggregations {
            occupied_types.insert(format!("{}_aggregate", model.table_name));
            occupied_types.insert(format!("{}_aggregate_fields", model.table_name));
        }
    }
    for command in commands.iter_mut() {
        if command.command_name.trim().is_empty() {
            return Err("command id must not be empty".into());
        }
        if !names.insert(command.command_name.clone()) {
            return Err(format!("duplicate command id `{}`", command.command_name));
        }
        if !is_valid_graphql_name(&command.field_name) {
            return Err(format!(
                "command `{}` mutation field `{}` is not a valid GraphQL name",
                command.command_name, command.field_name
            ));
        }
        if !fields.insert(command.field_name.clone()) {
            return Err(format!(
                "duplicate command mutation field `{}`",
                command.field_name
            ));
        }
        validate_nonempty_unique_ids(
            &command.roles,
            &format!("command `{}` role", command.command_name),
        )?;
        command.roles.sort();
        match &mut command.input {
            SurfaceCommandShape::Typed(definition) => {
                canonicalize_type_def(definition)?;
                reject_occupied_command_types(definition, &occupied_types)?;
                register_type_def(definition, true, &mut type_defs)?;
            }
            SurfaceCommandShape::None => {}
        }
        let output_command_name = command.command_name.clone();
        let output_consistency = command.consistency;
        let output_projected_model = command.projected_model.clone();
        match &mut command.output {
            SurfaceCommandShape::None => {
                return Err(format!(
                    "command `{}` cannot declare an empty output",
                    command.command_name
                ));
            }
            SurfaceCommandShape::Typed(definition) => {
                canonicalize_type_def(definition)?;
                if projected_output_reuses_surface_model(
                    &output_command_name,
                    output_consistency,
                    output_projected_model.as_ref(),
                    definition,
                    models,
                )? {
                    // `Projected<M>` deliberately returns the already-exposed
                    // normalized model object. Do not claim or re-emit a second
                    // GraphQL type with the same name.
                } else {
                    reject_occupied_command_types(definition, &occupied_types)?;
                    register_type_def(definition, false, &mut type_defs)?;
                }
            }
        }
        command
            .input_defaults
            .sort_by(|left, right| left.path.cmp(&right.path));
        validate_command_input_defaults(command)?;
        validate_command_effects(models, command)?;
        validate_command_confirmations(models, command)?;
        command.confirmations.sort_by(|left, right| {
            serde_json::to_string(&left.canonical_value())
                .expect("confirmation IR serialization cannot fail")
                .cmp(
                    &serde_json::to_string(&right.canonical_value())
                        .expect("confirmation IR serialization cannot fail"),
                )
        });
    }
    commands.sort_by(|a, b| a.command_name.cmp(&b.command_name));
    Ok(())
}

pub(crate) fn projected_output_reuses_surface_model(
    command_name: &str,
    consistency: CommandConsistency,
    projected: Option<&CommandProjectedModel>,
    definition: &SurfaceTypeDef,
    models: &BTreeMap<String, SurfaceModel>,
) -> Result<bool, String> {
    if consistency != CommandConsistency::Projected {
        return Ok(false);
    }
    let Some(projected) = projected else {
        return Ok(false);
    };
    let Some(model) = models.get(&projected.model) else {
        return Ok(false);
    };
    if definition.name != model.object_name {
        return Ok(false);
    }
    if definition.fields.len() != model.columns.len() {
        return Err(format!(
            "typed projected command `{}` output `{}` does not match the normalized Surface model columns",
            command_name, definition.name
        ));
    }
    for field in &definition.fields {
        let Some(column) = model
            .columns
            .iter()
            .find(|column| column.name == field.name)
        else {
            return Err(format!(
                "typed projected command `{}` output `{}` contains non-model field `{}`",
                command_name, definition.name, field.name
            ));
        };
        if field.type_name != column.scalar
            || field.nullable != column.nullable
            || field.list
            || field.item_nullable
            || field.nested.is_some()
        {
            return Err(format!(
                "typed projected command `{}` output field `{}.{}` differs from its normalized Surface model column",
                command_name, definition.name, field.name
            ));
        }
    }
    Ok(true)
}

pub(in crate::graphql::surface) fn validate_command_input_defaults(
    command: &SurfaceCommand,
) -> Result<(), String> {
    if command.input_defaults.is_empty() {
        return Ok(());
    }
    let SurfaceCommandShape::Typed(input) = &command.input else {
        return Err(format!(
            "typed command `{}` declares generated input defaults on an untyped input",
            command.command_name
        ));
    };
    let mut paths = BTreeSet::new();
    for default in &command.input_defaults {
        if default.path.len() != 1 {
            return Err(format!(
                "typed command `{}` generated input default must target exactly one top-level field",
                command.command_name
            ));
        }
        if !paths.insert(default.path.clone()) {
            return Err(format!(
                "typed command `{}` repeats generated input default `{}`",
                command.command_name,
                default.path.join(".")
            ));
        }
        let field_name = &default.path[0];
        let field = input
            .fields
            .iter()
            .find(|field| field.name == *field_name)
            .ok_or_else(|| {
                format!(
                    "typed command `{}` generated input default references unknown field `{field_name}`",
                    command.command_name
                )
            })?;
        if field.nullable
            || field.list
            || field.nested.is_some()
            || !matches!(field.type_name.as_str(), "String" | "ID")
        {
            return Err(format!(
                "typed command `{}` generated input default `{field_name}` requires a non-null, non-list String/ID field",
                command.command_name
            ));
        }
    }
    Ok(())
}

pub(in crate::graphql::surface) fn validate_command_confirmations(
    models: &BTreeMap<String, SurfaceModel>,
    command: &SurfaceCommand,
) -> Result<(), String> {
    validate_projection_confirmation_count(&command.command_name, command.confirmations.len())?;
    match command.consistency {
        CommandConsistency::Fact if command.confirmations.is_empty() => {
            return Err(format!(
                "typed fact command `{}` must declare at least one expected projector confirmation",
                command.command_name
            ));
        }
        CommandConsistency::Projected if !command.confirmations.is_empty() => {
            return Err(format!(
                "typed projected command `{}` cannot declare asynchronous projector confirmations",
                command.command_name
            ));
        }
        CommandConsistency::Projected if command.projected_model.is_none() => {
            return Err(format!(
                "typed projected command `{}` is missing its compiler-retained relational model",
                command.command_name
            ));
        }
        CommandConsistency::Accepted | CommandConsistency::Fact
            if command.projected_model.is_some() || command.direct_projection.is_some() =>
        {
            return Err(format!(
                "typed non-projected command `{}` cannot carry direct projection metadata",
                command.command_name
            ));
        }
        _ => {}
    }
    if let Some(projected) = &command.projected_model {
        let model = models.get(&projected.model).ok_or_else(|| {
            format!(
                "typed projected command `{}` output references unknown model `{}`",
                command.command_name, projected.model
            )
        })?;
        if model.table_name != projected.table {
            return Err(format!(
                "typed projected command `{}` output model `{}` resolves to table `{}`, not `{}`",
                command.command_name, projected.model, model.table_name, projected.table
            ));
        }
        if let Some(partition) = &projected.partition {
            validate_effect_expression(
                command,
                partition,
                &ColumnField {
                    name: "projector partition".into(),
                    scalar: "String".into(),
                    nullable: false,
                },
            )?;
        }
    }
    if let Some(target) = &command.direct_projection {
        let model = models.get(&target.model).ok_or_else(|| {
            format!(
                "typed projected command `{}` targets unknown model `{}`",
                command.command_name, target.model
            )
        })?;
        if model.table_name != target.table {
            return Err(format!(
                "typed projected command `{}` target model `{}` resolves to table `{}`, not `{}`",
                command.command_name, target.model, model.table_name, target.table
            ));
        }
        if let Some(partition) = &target.partition {
            validate_effect_expression(
                command,
                partition,
                &ColumnField {
                    name: "projector partition".into(),
                    scalar: "String".into(),
                    nullable: false,
                },
            )?;
        }
    }
    if command.confirmation_unavailable {
        return Err(format!(
            "catalog command `{}` cannot start with an unavailable confirmation plan",
            command.command_name
        ));
    }

    let mut seen = BTreeSet::new();
    for confirmation in &command.confirmations {
        if confirmation.projector.trim().is_empty() {
            return Err(format!(
                "typed command `{}` confirmation projector must not be empty",
                command.command_name
            ));
        }
        validate_effect_key(models, command, &confirmation.model, &confirmation.key)?;
        if let Some(partition) = &confirmation.partition {
            validate_effect_expression(
                command,
                partition,
                &ColumnField {
                    name: "projector partition".into(),
                    scalar: "String".into(),
                    nullable: false,
                },
            )?;
        }
        let identity =
            serde_json::to_string(confirmation).expect("confirmation IR serialization cannot fail");
        if !seen.insert(identity) {
            return Err(format!(
                "typed command `{}` repeats an expected projector confirmation",
                command.command_name
            ));
        }
    }
    Ok(())
}

pub(in crate::graphql::surface) fn bind_surface_direct_projection_targets(
    commands: &mut [SurfaceCommand],
    projectors: &[SurfaceProjectionOwner],
    models: &BTreeMap<String, SurfaceModel>,
) -> Result<(), String> {
    let mut compiled_projectors = BTreeMap::new();
    for projector in projectors {
        let schemas = projector
            .models
            .iter()
            .map(|model_name| {
                models
                    .get(model_name)
                    .map(|model| &model.schema)
                    .ok_or_else(|| {
                        format!(
                            "projector `{}` references unknown model `{model_name}`",
                            projector.name
                        )
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let compiled = compile_projection_topology(
            &projector.name,
            &projector.facts,
            &projector.models,
            &projector.partition,
            schemas,
        )
        .map_err(|error| {
            format!(
                "projector `{}` has invalid compiled topology: {error}",
                projector.name
            )
        })?;
        compiled_projectors.insert(projector.name.clone(), compiled);
    }

    for command in commands {
        for confirmation in &mut command.confirmations {
            let projector = projectors
                .iter()
                .find(|projector| projector.name == confirmation.projector)
                .ok_or_else(|| {
                    format!(
                        "typed command `{}` expects unknown projector `{}`",
                        command.command_name, confirmation.projector
                    )
                })?;
            if !confirmation.topology_matches(
                &projector.name,
                &projector.facts,
                &projector.models,
                &projector.partition,
            ) {
                return Err(format!(
                    "typed command `{}` captured projector `{}` topology identity does not match the registered projector facts/models",
                    command.command_name, confirmation.projector
                ));
            }
            if !confirmation.partition_matches(&projector.partition) {
                return Err(format!(
                    "typed command `{}` confirmation for projector `{}` does not provide the partition mapping required by its declaration",
                    command.command_name, confirmation.projector
                ));
            }
            if !projector
                .models
                .iter()
                .any(|model| model == &confirmation.model)
            {
                return Err(format!(
                    "typed command `{}` expects projector `{}` to confirm model `{}`, but that model is not in the projector topology",
                    command.command_name, confirmation.projector, confirmation.model
                ));
            }
            let (topology, _) = compiled_projectors
                .get(&projector.name)
                .expect("every registered projector was compiled above");
            confirmation.bind_protocol_topology(topology.clone());
        }

        if command.consistency != CommandConsistency::Projected {
            continue;
        }
        let projected = command.projected_model.as_ref().ok_or_else(|| {
            format!(
                "typed projected command `{}` is missing its compiler-retained relational model",
                command.command_name
            )
        })?;
        let owners = projectors
            .iter()
            .filter(|projector| {
                projector
                    .models
                    .iter()
                    .any(|model| model == &projected.model)
            })
            .collect::<Vec<_>>();
        let projector = match owners.as_slice() {
            [projector] => *projector,
            [] => {
                return Err(format!(
                    "typed projected command `{}` output model `{}` has no registered SurfaceProjector owner",
                    command.command_name, projected.model
                ))
            }
            _ => {
                return Err(format!(
                    "typed projected command `{}` output model `{}` has ambiguous SurfaceProjector ownership: {}",
                    command.command_name,
                    projected.model,
                    owners
                        .iter()
                        .map(|owner| owner.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            }
        };
        if projector.change_epoch.is_none() {
            return Err(format!(
                "typed projected command `{}` owner `{}` has no registered change-log epoch",
                command.command_name, projector.name
            ));
        }
        let registered_schema = &models
            .get(&projected.model)
            .expect("projector ownership above requires a registered model")
            .schema;
        if projected.schema != registered_schema {
            return Err(format!(
                "typed projected command `{}` retained schema for `{}` differs from the registered full table schema",
                command.command_name, projected.model
            ));
        }
        if !projected.partition_matches(&projector.partition) {
            return Err(format!(
                "typed projected command `{}` does not provide the partition mapping required by projector `{}`",
                command.command_name, projector.name
            ));
        }
        let (protocol_topology, ownership) = compiled_projectors
            .get(&projector.name)
            .expect("every registered projector was compiled above");
        command.direct_projection = Some(projected.bind(
            &projector.name,
            &projector.facts,
            &projector.models,
            &projector.partition,
            projector.change_epoch.as_deref(),
            ownership.clone(),
            Some(protocol_topology.clone()),
        ));
    }
    Ok(())
}

pub(in crate::graphql::surface) fn validate_command_confirmation_topology(
    commands: &[SurfaceCommand],
    projectors: &[SurfaceProjectionOwner],
    models: &BTreeMap<String, SurfaceModel>,
) -> Result<(), String> {
    let mut compiled_projectors = BTreeMap::new();
    let mut physical_owners = BTreeMap::new();
    for projector in projectors {
        let schemas = projector
            .models
            .iter()
            .map(|model_name| {
                models
                    .get(model_name)
                    .map(|model| &model.schema)
                    .ok_or_else(|| {
                        format!(
                            "projector `{}` references unknown model `{model_name}`",
                            projector.name
                        )
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let compiled = compile_projection_topology(
            &projector.name,
            &projector.facts,
            &projector.models,
            &projector.partition,
            schemas,
        )
        .map_err(|error| {
            format!(
                "projector `{}` has invalid compiled topology: {error}",
                projector.name
            )
        })?;
        for owner in &compiled.1 {
            if let Some((existing_projector, existing_model)) = physical_owners.insert(
                owner.table.clone(),
                (projector.name.clone(), owner.model.clone()),
            ) {
                return Err(format!(
                    "physical table `{}` has multiple projector owners: `{existing_projector}`/`{existing_model}` and `{}`/`{}`",
                    owner.table, projector.name, owner.model
                ));
            }
        }
        compiled_projectors.insert(projector.name.clone(), compiled);
    }

    for command in commands {
        for confirmation in &command.confirmations {
            let projector = projectors
                .iter()
                .find(|projector| projector.name == confirmation.projector)
                .ok_or_else(|| {
                    format!(
                        "typed command `{}` expects unknown projector `{}`",
                        command.command_name, confirmation.projector
                    )
                })?;
            if !projector
                .models
                .iter()
                .any(|model| model == &confirmation.model)
            {
                return Err(format!(
                    "typed command `{}` expects projector `{}` to confirm model `{}`, but that model is not in the projector topology",
                    command.command_name, confirmation.projector, confirmation.model
                ));
            }
            if !confirmation.topology_matches(
                &projector.name,
                &projector.facts,
                &projector.models,
                &projector.partition,
            ) {
                return Err(format!(
                    "typed command `{}` captured projector `{}` topology identity does not match the registered projector facts/models",
                    command.command_name, confirmation.projector
                ));
            }
            let (expected_topology, _) = compiled_projectors
                .get(&projector.name)
                .expect("every registered projector was compiled above");
            if confirmation.protocol_topology() != Some(expected_topology) {
                return Err(format!(
                    "typed command `{}` confirmation for projector `{}` is not bound to the exact compiled schema topology",
                    command.command_name, confirmation.projector
                ));
            }
        }
        if let Some(target) = &command.direct_projection {
            let projector = projectors
                .iter()
                .find(|projector| projector.name == target.projector)
                .ok_or_else(|| {
                    format!(
                        "typed projected command `{}` expects unknown direct projector `{}`",
                        command.command_name, target.projector
                    )
                })?;
            if !projector.models.iter().any(|model| model == &target.model) {
                return Err(format!(
                    "typed projected command `{}` direct projector `{}` does not own model `{}`",
                    command.command_name, target.projector, target.model
                ));
            }
            if !target.topology_matches(
                &projector.name,
                &projector.facts,
                &projector.models,
                &projector.partition,
                projector.change_epoch.as_deref(),
            ) {
                return Err(format!(
                    "typed projected command `{}` captured direct projector `{}` topology/change epoch does not match the registered owner",
                    command.command_name, target.projector
                ));
            }
            let (expected_topology, _) = compiled_projectors
                .get(&projector.name)
                .expect("every registered projector was compiled above");
            if !target.protocol_topology_matches(expected_topology) {
                return Err(format!(
                    "typed projected command `{}` direct projector `{}` is not bound to the exact compiled schema topology",
                    command.command_name, target.projector
                ));
            }
            if projector.change_epoch.is_none() {
                return Err(format!(
                    "typed projected command `{}` direct projector `{}` has no registered change-log epoch",
                    command.command_name, target.projector
                ));
            }
            let mut expected_ownership = projector
                .models
                .iter()
                .map(|model_name| {
                    let model = models.get(model_name).ok_or_else(|| {
                        format!(
                            "typed projected command `{}` owner `{}` references unknown model `{model_name}`",
                            command.command_name, projector.name
                        )
                    })?;
                    ProjectionModelOwnership::new(model_name, &model.table_name)
                        .map_err(|error| error.to_string())
                })
                .collect::<Result<Vec<_>, _>>()?;
            expected_ownership.sort_by(|left, right| {
                (left.model.as_str(), left.table.as_str())
                    .cmp(&(right.model.as_str(), right.table.as_str()))
            });
            if target.ownership != expected_ownership {
                return Err(format!(
                    "typed projected command `{}` direct projector `{}` captured an incomplete or stale model/table ownership inventory",
                    command.command_name, target.projector
                ));
            }

            let physical_owners = projectors
                .iter()
                .flat_map(|candidate| {
                    candidate.models.iter().filter_map(move |model_name| {
                        models
                            .get(model_name)
                            .filter(|model| model.table_name == target.table)
                            .map(|_| (candidate.name.as_str(), model_name.as_str()))
                    })
                })
                .collect::<Vec<_>>();
            if physical_owners.as_slice() != [(target.projector.as_str(), target.model.as_str())] {
                return Err(format!(
                    "typed projected command `{}` model `{}` has ambiguous direct projection ownership",
                    command.command_name, target.model
                ));
            }
        }
    }
    Ok(())
}
