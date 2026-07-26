use super::*;

pub(super) fn command_direct_projection_extension(
    command: &SurfaceCommand,
    surface: &Surface,
) -> Result<Option<CommandDirectProjectionExtension>, ClientManifestError> {
    let projected = command.consistency == CommandConsistency::Projected;
    let Some(target) = command.direct_projection.as_ref() else {
        return if projected {
            Err(ClientManifestError(format!(
                "typed projected command `{}` is missing its bound direct projection target",
                command.command_name
            )))
        } else {
            Ok(None)
        };
    };
    if !projected {
        return Err(ClientManifestError(format!(
            "typed non-projected command `{}` cannot export a direct projection target",
            command.command_name
        )));
    }
    let retained = command.projected_model.as_ref().ok_or_else(|| {
        ClientManifestError(format!(
            "typed projected command `{}` is missing its retained relational model",
            command.command_name
        ))
    })?;
    if target.model != retained.model {
        return Err(ClientManifestError(format!(
            "typed projected command `{}` direct target model `{}` differs from retained model `{}`",
            command.command_name, target.model, retained.model
        )));
    }
    let model = surface.models.get(&target.model).ok_or_else(|| {
        ClientManifestError(format!(
            "typed projected command `{}` direct target model `{}` is not authorized on this client surface",
            command.command_name, target.model
        ))
    })?;
    if !model_has_client_normalized_identity(model) {
        return Err(ClientManifestError(format!(
            "typed projected command `{}` direct target model `{}` has no complete authorized client identity",
            command.command_name, target.model
        )));
    }
    let SurfaceCommandShape::Typed(output) = &command.output else {
        return Err(ClientManifestError(format!(
            "typed projected command `{}` must return its exact relational model object",
            command.command_name
        )));
    };
    if output.name != model.object_name || output.fields.len() != model.columns.len() {
        return Err(ClientManifestError(format!(
            "typed projected command `{}` output does not match authorized model `{}`",
            command.command_name, target.model
        )));
    }
    for field in &output.fields {
        let matches = model.columns.iter().any(|column| {
            field.name == column.name
                && field.type_name == column.scalar
                && field.nullable == column.nullable
                && !field.list
                && !field.item_nullable
                && field.nested.is_none()
        });
        if !matches {
            return Err(ClientManifestError(format!(
                "typed projected command `{}` output field `{}.{}` differs from authorized model `{}`",
                command.command_name, output.name, field.name, target.model
            )));
        }
    }

    let topology = target.protocol_topology().ok_or_else(|| {
        ClientManifestError(format!(
            "typed projected command `{}` direct target is not bound to its compiled protocol topology",
            command.command_name
        ))
    })?;
    if topology.name() != target.projector {
        return Err(ClientManifestError(format!(
            "typed projected command `{}` direct target projector `{}` differs from bound topology `{}`",
            command.command_name,
            target.projector,
            topology.name()
        )));
    }

    let visible_owners = surface
        .projectors
        .iter()
        .filter(|projector| projector.models.iter().any(|model| model == &target.model))
        .collect::<Vec<_>>();
    match visible_owners.as_slice() {
        [] => {
            if surface
                .projectors
                .iter()
                .any(|projector| projector.name == topology.name())
            {
                return Err(ClientManifestError(format!(
                    "typed projected command `{}` topology `{}` does not own model `{}` on this client surface",
                    command.command_name,
                    topology.name(),
                    target.model
                )));
            }
            // A role surface may omit the whole projector when the same
            // topology also owns a denied model. The digest remains safe and
            // exact without revealing that hidden ownership inventory.
        }
        [owner] if owner.name == topology.name() => {
            if !target.topology_matches(
                &owner.name,
                &owner.facts,
                &owner.models,
                &owner.partition,
                owner.change_epoch.as_deref(),
            ) || !target.protocol_topology_matches(topology)
            {
                return Err(ClientManifestError(format!(
                    "typed projected command `{}` direct target differs from visible owner `{}`",
                    command.command_name, owner.name
                )));
            }
        }
        [owner] => {
            return Err(ClientManifestError(format!(
                "typed projected command `{}` names topology `{}` but visible owner `{}` owns model `{}`",
                command.command_name,
                topology.name(),
                owner.name,
                target.model
            )));
        }
        owners => {
            return Err(ClientManifestError(format!(
                "typed projected command `{}` model `{}` has ambiguous visible ownership: {}",
                command.command_name,
                target.model,
                owners
                    .iter()
                    .map(|owner| owner.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
    }

    let change_epoch = target.change_epoch.clone().ok_or_else(|| {
        ClientManifestError(format!(
            "typed projected command `{}` direct target has no registered change-log epoch",
            command.command_name
        ))
    })?;
    let partition = target
        .partition
        .as_ref()
        .map(|partition| serde_json::to_value(partition).map(canonical_json_value))
        .transpose()?;
    let digest = topology
        .digest()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(Some(CommandDirectProjectionExtension {
        topology: ClientProjectionTopologyIdentity {
            version: topology.version(),
            name: topology.name().to_string(),
            digest: format!("sha256:{digest}"),
        },
        model: target.model.clone(),
        partition,
        change_epoch,
    }))
}
pub(super) fn command_trusted_preset_descriptors(
    command: &SurfaceCommand,
    surface: &Surface,
) -> Result<Vec<ClientTrustedPresetDescriptor>, ClientManifestError> {
    fn register(
        out: &mut BTreeMap<String, String>,
        expression: &EffectExpression,
        codec: &str,
        command: &SurfaceCommand,
    ) -> Result<(), ClientManifestError> {
        let EffectExpression::TrustedPreset { name } = expression else {
            return Ok(());
        };
        match out.entry(name.clone()) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(codec.to_string());
            }
            std::collections::btree_map::Entry::Occupied(entry) if entry.get() == codec => {}
            std::collections::btree_map::Entry::Occupied(entry) => {
                return Err(ClientManifestError(format!(
                    "command `{}` trusted preset `{name}` is used with incompatible codecs `{}` and `{codec}`",
                    command.command_name,
                    entry.get()
                )));
            }
        }
        Ok(())
    }

    fn field_codec<'a>(
        surface: &'a Surface,
        model: &str,
        field: &str,
    ) -> Result<&'static str, ClientManifestError> {
        let column = surface
            .models
            .get(model)
            .and_then(|model| model.columns.iter().find(|column| column.name == field))
            .ok_or_else(|| {
                ClientManifestError(format!(
                    "trusted preset target references absent field `{model}.{field}`"
                ))
            })?;
        scalar_codec(&column.scalar).ok_or_else(|| {
            ClientManifestError(format!(
                "trusted preset target `{model}.{field}` uses unsupported scalar `{}`",
                column.scalar
            ))
        })
    }

    fn collect_key(
        out: &mut BTreeMap<String, String>,
        key: &EffectKey,
        model: &str,
        command: &SurfaceCommand,
        surface: &Surface,
    ) -> Result<(), ClientManifestError> {
        for field in &key.fields {
            register(
                out,
                &field.value,
                field_codec(surface, model, &field.field)?,
                command,
            )?;
        }
        Ok(())
    }

    let mut out = BTreeMap::new();
    if let Some(effects) = &command.effects {
        for operation in &effects.operations {
            match operation {
                CommandEffect::Upsert { model, key, fields }
                | CommandEffect::Patch { model, key, fields } => {
                    collect_key(&mut out, key, model, command, surface)?;
                    for field in fields {
                        register(
                            &mut out,
                            &field.value,
                            field_codec(surface, model, &field.field)?,
                            command,
                        )?;
                    }
                }
                CommandEffect::Delete { model, key } => {
                    collect_key(&mut out, key, model, command, surface)?;
                }
                CommandEffect::Link {
                    relationship,
                    source,
                    target,
                }
                | CommandEffect::Unlink {
                    relationship,
                    source,
                    target,
                } => {
                    collect_key(
                        &mut out,
                        source,
                        &relationship.source_model,
                        command,
                        surface,
                    )?;
                    collect_key(
                        &mut out,
                        target,
                        &relationship.target_model,
                        command,
                        surface,
                    )?;
                }
                CommandEffect::InvalidateRelationship {
                    relationship,
                    source,
                } => {
                    collect_key(
                        &mut out,
                        source,
                        &relationship.source_model,
                        command,
                        surface,
                    )?;
                }
                CommandEffect::InvalidateModel { .. } => {}
            }
        }
    }
    for confirmation in &command.confirmations {
        collect_key(
            &mut out,
            &confirmation.key,
            &confirmation.model,
            command,
            surface,
        )?;
        if let Some(partition) = &confirmation.partition {
            register(&mut out, partition, "string", command)?;
        }
    }
    if let Some(direct) = &command.direct_projection {
        if let Some(partition) = &direct.partition {
            register(&mut out, partition, "string", command)?;
        }
    }

    Ok(out
        .into_iter()
        .map(|(name, codec)| ClientTrustedPresetDescriptor { name, codec })
        .collect())
}
pub(super) fn command_operation(
    mutation_field: &str,
    input: &ClientCommandShape,
    output: &ClientCommandShape,
) -> String {
    let operation_name = format!("Client_{mutation_field}");
    let (variables, arguments) = match input {
        ClientCommandShape::None => (
            "($commandId: ID!)".to_string(),
            "(commandId: $commandId)".to_string(),
        ),
        ClientCommandShape::Object { definition } => (
            format!("($commandId: ID!, $input: {}!)", definition.name),
            "(commandId: $commandId, input: $input)".to_string(),
        ),
    };
    let selection = match output {
        ClientCommandShape::Object { definition } => {
            format!(" {{ {} }}", command_selection(definition))
        }
        ClientCommandShape::None => String::new(),
    };
    format!("mutation {operation_name}{variables} {{ {mutation_field}{arguments}{selection} }}")
}

pub(super) fn command_status_operation() -> String {
    "query Distributed_CommandStatus($commandId: ID!) { commandStatus(commandId: $commandId) { state } }"
        .into()
}

fn command_selection(definition: &ClientTypeDef) -> String {
    definition
        .fields
        .iter()
        .map(|field| match &field.nested {
            Some(nested) => format!("{} {{ {} }}", field.name, command_selection(nested)),
            None => field.name.clone(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}
