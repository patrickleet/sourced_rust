use std::collections::{BTreeMap, BTreeSet};

use base64::Engine as _;
use serde_json::Value as JsonValue;

use super::manifest::{
    validate_exact_operation_hash, ManifestCommand, ManifestCommandShape, ManifestConfirmationKind,
    ManifestEffect, ManifestEffectExpression, ManifestEffectField, ManifestEffectKey,
    ManifestEffectRelationship, ManifestField, ManifestInputDefault, ManifestModel,
    ManifestNormalization, ManifestProjector, ManifestProtocolOperations, ManifestTypeDef,
    ManifestTypeField,
};
use super::ClientCompileError;

const COMMAND_STATUS_OPERATION: &str =
    "query Distributed_CommandStatus($commandId: ID!) { commandStatus(commandId: $commandId) { state } }";
// Mirrors `projection_protocol::MAX_PROJECTION_EVIDENCE_BATCH_ITEMS`, which is
// not exported across the `distributed`/`distributed_cli` package boundary.
const MAX_CONFIRMATIONS: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CommandTypeKind {
    Input,
    Output,
}

/// Semantic command validation which cannot be expressed by serde alone.
///
/// A command is included in `commands_requiring_revalidation` when its
/// authorized manifest intentionally withholds an exact optimistic effect or
/// finite confirmation plan.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct CommandManifestValidation {
    pub(crate) commands_requiring_revalidation: BTreeSet<String>,
}

pub(crate) fn validate_command_manifest(
    commands: &[ManifestCommand],
    models: &BTreeMap<String, ManifestModel>,
    scalar_codecs: &BTreeMap<String, String>,
    projectors: &[ManifestProjector],
    causal_receipts: bool,
    protocol_operations: &ManifestProtocolOperations,
) -> Result<CommandManifestValidation, ClientCompileError> {
    if causal_receipts != !commands.is_empty() {
        return Err(invalid(
            "client.manifest.command_capability",
            "manifest command inventory and capabilities.causal_receipts must agree",
        ));
    }
    validate_protocol_operations(protocol_operations, !commands.is_empty())?;
    let projectors = validate_projector_inventory(projectors, models)?;

    let mut command_names = BTreeSet::new();
    let mut mutation_fields = BTreeSet::new();
    let mut type_definitions = BTreeMap::new();
    let mut report = CommandManifestValidation::default();
    for command in commands {
        validate_command(
            command,
            models,
            scalar_codecs,
            &projectors,
            &mut type_definitions,
            &mut report,
        )?;
        if !command_names.insert(command.name.as_str()) {
            return Err(invalid(
                "client.manifest.duplicate_command",
                format!("duplicate manifest command `{}`", command.name),
            ));
        }
        if !mutation_fields.insert(command.mutation_field.as_str()) {
            return Err(invalid(
                "client.manifest.duplicate_command_field",
                format!(
                    "duplicate manifest command mutation field `{}`",
                    command.mutation_field
                ),
            ));
        }
    }
    Ok(report)
}

fn validate_command(
    command: &ManifestCommand,
    models: &BTreeMap<String, ManifestModel>,
    scalar_codecs: &BTreeMap<String, String>,
    projectors: &BTreeMap<&str, &ManifestProjector>,
    type_definitions: &mut BTreeMap<String, (CommandTypeKind, ManifestTypeDef)>,
    report: &mut CommandManifestValidation,
) -> Result<(), ClientCompileError> {
    if command.version != 1 {
        return Err(command_error(
            command,
            "client.manifest.command_version",
            "version must be 1",
        ));
    }
    nonempty(&command.name, "command name")?;
    graphql_name(&command.mutation_field, "command mutation field")?;
    unique_nonempty(
        &command.grants,
        &format!("command `{}` grant", command.name),
    )?;

    validate_shape(
        &command.input,
        CommandTypeKind::Input,
        scalar_codecs,
        &format!("command `{}` input", command.name),
        type_definitions,
    )?;
    if matches!(command.output, ManifestCommandShape::None) {
        return Err(command_error(
            command,
            "client.manifest.command_output",
            "cannot declare an empty output",
        ));
    }
    validate_shape(
        &command.output,
        CommandTypeKind::Output,
        scalar_codecs,
        &format!("command `{}` output", command.name),
        type_definitions,
    )?;

    let canonical = canonical_command_operation(command);
    if command.operation != canonical {
        return Err(command_error(
            command,
            "client.manifest.command_operation",
            "operation does not byte-match the canonical typed command operation",
        ));
    }
    validate_exact_operation_hash(&command.operation, &command.operation_hash, "command")?;

    let extensions = &command.extensions;
    if extensions.version != 2 {
        return Err(command_error(
            command,
            "client.manifest.command_extensions",
            "extensions.version must be 2",
        ));
    }
    let consistency = extensions.consistency.as_ref().ok_or_else(|| {
        command_error(
            command,
            "client.manifest.command_consistency",
            "requires typed consistency metadata",
        )
    })?;
    if consistency.version != 1 {
        return Err(command_error(
            command,
            "client.manifest.command_consistency",
            "consistency.version must be 1",
        ));
    }
    if let Some(defaults) = &extensions.input_defaults {
        validate_defaults(command, defaults.version, &defaults.defaults)?;
    }
    if let Some(effects) = &extensions.effects {
        if effects.version != 1 {
            return Err(command_error(
                command,
                "client.manifest.command_effects",
                "effects.version must be 1",
            ));
        }
        if effects.operations.is_empty() {
            report
                .commands_requiring_revalidation
                .insert(command.name.clone());
        }
        for effect in &effects.operations {
            validate_effect(command, effect, models, report)?;
        }
    } else {
        report
            .commands_requiring_revalidation
            .insert(command.name.clone());
    }
    validate_confirmations(command, models, projectors, report)
}

fn validate_shape(
    shape: &ManifestCommandShape,
    kind: CommandTypeKind,
    scalar_codecs: &BTreeMap<String, String>,
    label: &str,
    definitions: &mut BTreeMap<String, (CommandTypeKind, ManifestTypeDef)>,
) -> Result<(), ClientCompileError> {
    match shape {
        ManifestCommandShape::None => Ok(()),
        ManifestCommandShape::Json { codec } => validate_codec("JSON", codec, scalar_codecs, label),
        ManifestCommandShape::Object { definition } => {
            validate_type_def(definition, kind, scalar_codecs, label, definitions)
        }
    }
}

fn validate_type_def(
    definition: &ManifestTypeDef,
    kind: CommandTypeKind,
    scalar_codecs: &BTreeMap<String, String>,
    label: &str,
    definitions: &mut BTreeMap<String, (CommandTypeKind, ManifestTypeDef)>,
) -> Result<(), ClientCompileError> {
    graphql_name(&definition.name, &format!("{label} type"))?;
    if let Some((previous_kind, previous)) = definitions.get(&definition.name) {
        return if *previous_kind == kind && previous == definition {
            Ok(())
        } else {
            Err(invalid(
                "client.manifest.command_type_reference",
                format!(
                    "GraphQL type `{}` has ambiguous input/output or structural definitions",
                    definition.name
                ),
            ))
        };
    }
    definitions.insert(definition.name.clone(), (kind, definition.clone()));
    if definition.fields.is_empty() {
        return Err(invalid(
            "client.manifest.command_type_fields",
            format!("{label} type `{}` must contain a field", definition.name),
        ));
    }
    let mut names = BTreeSet::new();
    let mut previous = None;
    for field in &definition.fields {
        graphql_name(&field.name, &format!("{label} field"))?;
        graphql_name(&field.type_name, &format!("{label} field type"))?;
        if !names.insert(field.name.as_str()) {
            return Err(invalid(
                "client.manifest.command_type_field",
                format!("{label} repeats field `{}`", field.name),
            ));
        }
        if previous.is_some_and(|name| name >= field.name.as_str()) {
            return Err(invalid(
                "client.manifest.command_type_field",
                format!("{label} fields must use canonical name order"),
            ));
        }
        previous = Some(field.name.as_str());
        if field.item_nullable && !field.list {
            return Err(invalid(
                "client.manifest.command_type_nullability",
                format!(
                    "{label} field `{}` marks a non-list item nullable",
                    field.name
                ),
            ));
        }
        match (&field.codec, &field.nested) {
            (Some(codec), None) => {
                validate_codec(&field.type_name, codec, scalar_codecs, label)?;
            }
            (None, Some(nested)) if nested.name == field.type_name => {
                validate_type_def(
                    nested,
                    kind,
                    scalar_codecs,
                    &format!("{label}.{}", field.name),
                    definitions,
                )?;
            }
            (None, Some(_)) => {
                return Err(invalid(
                    "client.manifest.command_type_reference",
                    format!(
                        "{label} field `{}` type does not match its nested definition",
                        field.name
                    ),
                ));
            }
            _ => {
                return Err(invalid(
                    "client.manifest.command_type_codec",
                    format!(
                        "{label} field `{}` must declare exactly one scalar codec or nested type",
                        field.name
                    ),
                ));
            }
        }
    }
    Ok(())
}

fn validate_codec(
    scalar: &str,
    codec: &str,
    scalar_codecs: &BTreeMap<String, String>,
    label: &str,
) -> Result<(), ClientCompileError> {
    match scalar_codecs.get(scalar) {
        Some(expected) if expected == codec => Ok(()),
        Some(expected) => Err(invalid(
            "client.manifest.command_type_codec",
            format!("{label} codec `{codec}` does not match `{scalar}` codec `{expected}`"),
        )),
        None => Err(invalid(
            "client.manifest.command_type_scalar",
            format!("{label} references unsupported scalar `{scalar}`"),
        )),
    }
}

fn canonical_command_operation(command: &ManifestCommand) -> String {
    let operation_name = format!("Client_{}", command.mutation_field);
    let (variables, arguments) = match &command.input {
        ManifestCommandShape::None => ("($commandId: ID!)".to_string(), "(commandId: $commandId)"),
        ManifestCommandShape::Json { .. } => (
            "($commandId: ID!, $input: JSON!)".to_string(),
            "(commandId: $commandId, input: $input)",
        ),
        ManifestCommandShape::Object { definition } => (
            format!("($commandId: ID!, $input: {}!)", definition.name),
            "(commandId: $commandId, input: $input)",
        ),
    };
    let selection = match &command.output {
        ManifestCommandShape::Object { definition } => {
            format!(" {{ {} }}", command_selection(definition))
        }
        ManifestCommandShape::None | ManifestCommandShape::Json { .. } => String::new(),
    };
    format!(
        "mutation {operation_name}{variables} {{ {}{arguments}{selection} }}",
        command.mutation_field
    )
}

fn command_selection(definition: &ManifestTypeDef) -> String {
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

fn validate_defaults(
    command: &ManifestCommand,
    version: u32,
    defaults: &[ManifestInputDefault],
) -> Result<(), ClientCompileError> {
    if version != 1 || defaults.is_empty() {
        return Err(command_error(
            command,
            "client.manifest.input_defaults",
            "input_defaults must be version 1 with at least one entry",
        ));
    }
    let ManifestCommandShape::Object { definition } = &command.input else {
        return Err(command_error(
            command,
            "client.manifest.input_default_path",
            "generated defaults require a typed object input",
        ));
    };
    let mut paths = BTreeSet::new();
    for default in defaults {
        let [field_name] = default.path.as_slice() else {
            return Err(command_error(
                command,
                "client.manifest.input_default_path",
                "generated default must target exactly one top-level input field",
            ));
        };
        if !paths.insert(field_name.as_str()) {
            return Err(command_error(
                command,
                "client.manifest.input_default_path",
                format!("repeats generated default path `{field_name}`"),
            ));
        }
        let field = definition
            .fields
            .iter()
            .find(|field| field.name == *field_name)
            .ok_or_else(|| {
                command_error(
                    command,
                    "client.manifest.input_default_path",
                    format!("generated default references unknown input field `{field_name}`"),
                )
            })?;
        if field.nullable
            || field.list
            || field.nested.is_some()
            || !matches!(field.type_name.as_str(), "String" | "ID")
        {
            return Err(command_error(
                command,
                "client.manifest.input_default_path",
                format!(
                    "generated default `{field_name}` requires a non-null, non-list String/ID field"
                ),
            ));
        }
    }
    Ok(())
}

fn validate_effect(
    command: &ManifestCommand,
    effect: &ManifestEffect,
    models: &BTreeMap<String, ManifestModel>,
    report: &mut CommandManifestValidation,
) -> Result<(), ClientCompileError> {
    match effect {
        ManifestEffect::Upsert { model, key, fields }
        | ManifestEffect::Patch { model, key, fields } => {
            let model = addressable_model(command, model, models)?;
            validate_key(command, model, key, false, report)?;
            validate_effect_fields(command, model, fields)
        }
        ManifestEffect::Delete { model, key } => {
            let model = addressable_model(command, model, models)?;
            validate_key(command, model, key, false, report)
        }
        ManifestEffect::Link {
            relationship,
            source,
            target,
        }
        | ManifestEffect::Unlink {
            relationship,
            source,
            target,
        } => {
            let (source_model, target_model) =
                validate_relationship(command, relationship, models)?;
            require_addressable(command, source_model)?;
            require_addressable(command, target_model)?;
            validate_key(command, source_model, source, false, report)?;
            validate_key(command, target_model, target, false, report)
        }
        ManifestEffect::InvalidateModel { model } => {
            require_model(command, model, models).map(|_| ())
        }
        ManifestEffect::InvalidateRelationship {
            relationship,
            source,
        } => {
            let (source_model, _) = validate_relationship(command, relationship, models)?;
            require_addressable(command, source_model)?;
            validate_key(command, source_model, source, false, report)
        }
    }
}

fn validate_effect_fields(
    command: &ManifestCommand,
    model: &ManifestModel,
    fields: &[ManifestEffectField],
) -> Result<(), ClientCompileError> {
    let identity = model
        .identity()
        .expect("effect model addressability checked before field validation");
    let mut names = BTreeSet::new();
    for assignment in fields {
        if !names.insert(assignment.field.as_str()) {
            return Err(command_error(
                command,
                "client.manifest.effect_field",
                format!("effect repeats `{}.{}`", model.id, assignment.field),
            ));
        }
        let field = model.field(&assignment.field).ok_or_else(|| {
            command_error(
                command,
                "client.manifest.effect_field",
                format!(
                    "effect references unknown field `{}.{}`",
                    model.id, assignment.field
                ),
            )
        })?;
        if identity.iter().any(|key| key.name == assignment.field) {
            return Err(command_error(
                command,
                "client.manifest.effect_field",
                format!(
                    "effect cannot assign identity field `{}.{}`",
                    model.id, field.name
                ),
            ));
        }
        validate_expression(command, &assignment.value, field)?;
    }
    Ok(())
}

fn validate_relationship<'a>(
    command: &ManifestCommand,
    relationship: &ManifestEffectRelationship,
    models: &'a BTreeMap<String, ManifestModel>,
) -> Result<(&'a ManifestModel, &'a ManifestModel), ClientCompileError> {
    let source = require_model(command, &relationship.source_model, models)?;
    let declared = source.relationship(&relationship.field).ok_or_else(|| {
        command_error(
            command,
            "client.manifest.effect_relationship",
            format!(
                "effect references unknown relationship `{}.{}`",
                relationship.source_model, relationship.field
            ),
        )
    })?;
    if declared.target_model != relationship.target_model {
        return Err(command_error(
            command,
            "client.manifest.effect_relationship",
            format!(
                "relationship `{}.{}` targets `{}`, not `{}`",
                relationship.source_model,
                relationship.field,
                declared.target_model,
                relationship.target_model
            ),
        ));
    }
    let target = require_model(command, &relationship.target_model, models)?;
    Ok((source, target))
}

fn validate_key(
    command: &ManifestCommand,
    model: &ManifestModel,
    key: &ManifestEffectKey,
    allow_embedded: bool,
    report: &mut CommandManifestValidation,
) -> Result<(), ClientCompileError> {
    match &model.normalization {
        ManifestNormalization::Normalized { fields, .. } => {
            let actual = key
                .fields
                .iter()
                .map(|field| field.field.as_str())
                .collect::<Vec<_>>();
            let expected = fields
                .iter()
                .map(|field| field.name.as_str())
                .collect::<Vec<_>>();
            if actual != expected {
                return Err(command_error(
                    command,
                    "client.manifest.effect_key",
                    format!(
                        "key for `{}` must exactly match ordered identity ({})",
                        model.id,
                        expected.join(", ")
                    ),
                ));
            }
        }
        ManifestNormalization::Embedded if allow_embedded => {
            report
                .commands_requiring_revalidation
                .insert(command.name.clone());
            if key.fields.is_empty() {
                return Err(command_error(
                    command,
                    "client.manifest.confirmation_key",
                    format!(
                        "embedded confirmation key for `{}` must not be empty",
                        model.id
                    ),
                ));
            }
            let mut names = BTreeSet::new();
            if key
                .fields
                .iter()
                .any(|field| !names.insert(field.field.as_str()))
            {
                return Err(command_error(
                    command,
                    "client.manifest.confirmation_key",
                    format!(
                        "embedded confirmation key for `{}` repeats a field",
                        model.id
                    ),
                ));
            }
        }
        ManifestNormalization::Embedded => {
            return Err(command_error(
                command,
                "client.manifest.effect_identity",
                format!(
                    "key-addressed effect cannot target embedded model `{}`",
                    model.id
                ),
            ));
        }
    }
    for key_field in &key.fields {
        let field = model.field(&key_field.field).ok_or_else(|| {
            command_error(
                command,
                "client.manifest.effect_key",
                format!(
                    "key for `{}` references unknown field `{}`",
                    model.id, key_field.field
                ),
            )
        })?;
        validate_expression(command, &key_field.value, field)?;
    }
    Ok(())
}

fn validate_expression(
    command: &ManifestCommand,
    expression: &ManifestEffectExpression,
    expected: &ManifestField,
) -> Result<(), ClientCompileError> {
    match expression {
        ManifestEffectExpression::Input { path } => {
            let (field, inherited_nullable) = input_field(command, path)?;
            let json_container =
                expected.scalar == "JSON" && (field.list || field.nested.is_some());
            if !json_container
                && (field.list || field.nested.is_some() || field.type_name != expected.scalar)
            {
                return Err(command_error(
                    command,
                    "client.manifest.effect_input_type",
                    format!(
                        "input `{}` cannot populate `{}:{}`",
                        path.join("."),
                        expected.name,
                        expected.scalar
                    ),
                ));
            }
            if (inherited_nullable || field.nullable) && !expected.nullable {
                return Err(command_error(
                    command,
                    "client.manifest.effect_input_nullability",
                    format!(
                        "nullable input `{}` cannot populate non-null field `{}`",
                        path.join("."),
                        expected.name
                    ),
                ));
            }
            Ok(())
        }
        ManifestEffectExpression::TrustedPreset { .. } => Err(command_error(
            command,
            "client.manifest.effect_trusted_preset",
            "uses a trusted preset before a cache-scope-bound server preset capability is installed",
        )),
        ManifestEffectExpression::Constant { value } if constant_matches(value, expected) => Ok(()),
        ManifestEffectExpression::Constant { .. } => Err(command_error(
            command,
            "client.manifest.effect_constant",
            format!(
                "constant is incompatible with field `{}` (`{}`)",
                expected.name, expected.scalar
            ),
        )),
        ManifestEffectExpression::Null if expected.nullable => Ok(()),
        ManifestEffectExpression::Null => Err(command_error(
            command,
            "client.manifest.effect_null",
            format!("null cannot populate non-null field `{}`", expected.name),
        )),
    }
}

fn input_field<'a>(
    command: &'a ManifestCommand,
    path: &[String],
) -> Result<(&'a ManifestTypeField, bool), ClientCompileError> {
    if path.is_empty() {
        return Err(command_error(
            command,
            "client.manifest.effect_input_path",
            "effect input path must not be empty",
        ));
    }
    let ManifestCommandShape::Object { definition } = &command.input else {
        return Err(command_error(
            command,
            "client.manifest.effect_input_path",
            "effect input references require a typed object input",
        ));
    };
    let mut current = definition;
    let mut inherited_nullable = false;
    for (index, segment) in path.iter().enumerate() {
        let field = current
            .fields
            .iter()
            .find(|field| field.name == *segment)
            .ok_or_else(|| {
                command_error(
                    command,
                    "client.manifest.effect_input_path",
                    format!("effect references unknown input path `{}`", path.join(".")),
                )
            })?;
        if index + 1 == path.len() {
            return Ok((field, inherited_nullable));
        }
        if field.list {
            return Err(command_error(
                command,
                "client.manifest.effect_input_path",
                format!(
                    "effect input path `{}` descends through a list",
                    path.join(".")
                ),
            ));
        }
        inherited_nullable |= field.nullable;
        current = field.nested.as_deref().ok_or_else(|| {
            command_error(
                command,
                "client.manifest.effect_input_path",
                format!(
                    "effect input path `{}` descends through a scalar",
                    path.join(".")
                ),
            )
        })?;
    }
    unreachable!("a non-empty path either resolves or returns an error")
}

fn validate_confirmations(
    command: &ManifestCommand,
    models: &BTreeMap<String, ManifestModel>,
    projectors: &BTreeMap<&str, &ManifestProjector>,
    report: &mut CommandManifestValidation,
) -> Result<(), ClientCompileError> {
    use super::manifest::ManifestConsistencyKind;

    let confirmations = command.extensions.confirmations.as_ref();
    match (
        command.extensions.consistency.as_ref().map(|c| c.kind),
        confirmations,
    ) {
        (Some(ManifestConsistencyKind::Fact), None) => {
            return Err(command_error(
                command,
                "client.manifest.command_confirmations",
                "fact consistency requires confirmations",
            ));
        }
        (Some(ManifestConsistencyKind::Projected), Some(_)) => {
            return Err(command_error(
                command,
                "client.manifest.command_confirmations",
                "projected consistency cannot declare asynchronous confirmations",
            ));
        }
        _ => {}
    }
    let Some(confirmations) = confirmations else {
        return Ok(());
    };
    if confirmations.version != 1 {
        return Err(command_error(
            command,
            "client.manifest.command_confirmations",
            "confirmations.version must be 1",
        ));
    }
    if confirmations.expected.len() > MAX_CONFIRMATIONS {
        return Err(command_error(
            command,
            "client.manifest.command_confirmations",
            format!(
                "declares {} projector confirmations; maximum is {MAX_CONFIRMATIONS}",
                confirmations.expected.len()
            ),
        ));
    }
    match confirmations.kind {
        ManifestConfirmationKind::Unavailable => {
            if !confirmations.expected.is_empty() {
                return Err(command_error(
                    command,
                    "client.manifest.command_confirmations",
                    "unavailable confirmations must have an empty expected inventory",
                ));
            }
            report
                .commands_requiring_revalidation
                .insert(command.name.clone());
            return Ok(());
        }
        ManifestConfirmationKind::Finite if confirmations.expected.is_empty() => {
            return Err(command_error(
                command,
                "client.manifest.command_confirmations",
                "finite confirmations must contain at least one expected target",
            ));
        }
        ManifestConfirmationKind::Finite => {}
    }

    let mut seen = BTreeSet::new();
    for confirmation in &confirmations.expected {
        let projector = projectors
            .get(confirmation.projector.as_str())
            .copied()
            .ok_or_else(|| {
                command_error(
                    command,
                    "client.manifest.confirmation_projector",
                    format!(
                        "confirmation references unknown projector `{}`",
                        confirmation.projector
                    ),
                )
            })?;
        if !projector.causal_confirmation
            || !projector
                .models
                .iter()
                .any(|model| model == &confirmation.model)
        {
            return Err(command_error(
                command,
                "client.manifest.confirmation_projector",
                format!(
                    "projector `{}` is not an authorized causal owner of model `{}`",
                    projector.name, confirmation.model
                ),
            ));
        }
        let model = require_model(command, &confirmation.model, models)?;
        validate_key(command, model, &confirmation.key, true, report)?;
        if let Some(partition) = &confirmation.partition {
            let expected = ManifestField {
                name: "projector partition".into(),
                scalar: "String".into(),
                codec: "string".into(),
                nullable: false,
            };
            validate_expression(command, partition, &expected)?;
        }
        let identity = serde_json::to_string(confirmation).map_err(|error| {
            command_error(
                command,
                "client.manifest.command_confirmations",
                format!("could not canonicalize confirmation: {error}"),
            )
        })?;
        if !seen.insert(identity) {
            return Err(command_error(
                command,
                "client.manifest.command_confirmations",
                "repeats an expected projector confirmation",
            ));
        }
    }
    Ok(())
}

fn validate_projector_inventory<'a>(
    projectors: &'a [ManifestProjector],
    models: &BTreeMap<String, ManifestModel>,
) -> Result<BTreeMap<&'a str, &'a ManifestProjector>, ClientCompileError> {
    let mut result = BTreeMap::new();
    for projector in projectors {
        if projector.version != 1 {
            return Err(invalid(
                "client.manifest.projector_version",
                format!("projector `{}` must use version 1", projector.name),
            ));
        }
        nonempty(&projector.name, "projector name")?;
        for model in &projector.models {
            if !models.contains_key(model) {
                return Err(invalid(
                    "client.manifest.projector_model",
                    format!(
                        "projector `{}` references unknown model `{model}`",
                        projector.name
                    ),
                ));
            }
        }
        if result.insert(projector.name.as_str(), projector).is_some() {
            return Err(invalid(
                "client.manifest.duplicate_projector",
                format!("duplicate projector `{}`", projector.name),
            ));
        }
    }
    Ok(result)
}

fn validate_protocol_operations(
    operations: &ManifestProtocolOperations,
    commands_present: bool,
) -> Result<(), ClientCompileError> {
    if operations.version != 1 {
        return Err(invalid(
            "client.manifest.protocol_operations",
            "manifest.protocol_operations.version must be 1",
        ));
    }
    match (&operations.command_status, commands_present) {
        (None, true) => Err(invalid(
            "client.manifest.command_status",
            "manifest commands require the framework command-status operation",
        )),
        (Some(_), false) => Err(invalid(
            "client.manifest.command_status",
            "query-only manifests must not expose a command-status operation",
        )),
        (None, false) => Ok(()),
        (Some(status), true) => {
            if status.name != "Distributed_CommandStatus"
                || status.operation != COMMAND_STATUS_OPERATION
            {
                return Err(invalid(
                    "client.manifest.command_status",
                    "command-status operation does not byte-match the framework contract",
                ));
            }
            validate_exact_operation_hash(
                &status.operation,
                &status.operation_hash,
                "command status",
            )
        }
    }
}

fn addressable_model<'a>(
    command: &ManifestCommand,
    name: &str,
    models: &'a BTreeMap<String, ManifestModel>,
) -> Result<&'a ManifestModel, ClientCompileError> {
    let model = require_model(command, name, models)?;
    require_addressable(command, model)?;
    Ok(model)
}

fn require_addressable(
    command: &ManifestCommand,
    model: &ManifestModel,
) -> Result<(), ClientCompileError> {
    if model.identity().is_some_and(|fields| !fields.is_empty()) {
        Ok(())
    } else {
        Err(command_error(
            command,
            "client.manifest.effect_identity",
            format!(
                "key-addressed effect cannot target embedded model `{}`",
                model.id
            ),
        ))
    }
}

fn require_model<'a>(
    command: &ManifestCommand,
    name: &str,
    models: &'a BTreeMap<String, ManifestModel>,
) -> Result<&'a ManifestModel, ClientCompileError> {
    models.get(name).ok_or_else(|| {
        command_error(
            command,
            "client.manifest.effect_model",
            format!("references unknown model `{name}`"),
        )
    })
}

fn constant_matches(value: &JsonValue, expected: &ManifestField) -> bool {
    if expected.scalar == "JSON" {
        return true;
    }
    match (expected.scalar.as_str(), value) {
        ("Boolean", JsonValue::Bool(_)) => true,
        ("BigInt", JsonValue::Number(number)) => number.is_i64() || number.is_u64(),
        ("Int", JsonValue::Number(number)) => {
            number
                .as_i64()
                .is_some_and(|value| i32::try_from(value).is_ok())
                || number
                    .as_u64()
                    .is_some_and(|value| i32::try_from(value).is_ok())
        }
        ("Float", JsonValue::Number(_)) => true,
        ("String" | "ID", JsonValue::String(_)) => true,
        ("Timestamptz", JsonValue::String(value)) => is_rfc3339(value),
        ("Bytea", JsonValue::String(value)) => base64::engine::general_purpose::STANDARD
            .decode(value)
            .is_ok(),
        _ => false,
    }
}

fn is_rfc3339(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() < 20
        || bytes.get(4) != Some(&b'-')
        || bytes.get(7) != Some(&b'-')
        || !matches!(bytes.get(10), Some(b'T' | b't'))
        || bytes.get(13) != Some(&b':')
        || bytes.get(16) != Some(&b':')
    {
        return false;
    }
    let number = |range: std::ops::Range<usize>| {
        std::str::from_utf8(bytes.get(range)?)
            .ok()?
            .parse::<u32>()
            .ok()
    };
    let (Some(year), Some(month), Some(day), Some(hour), Some(minute), Some(second)) = (
        number(0..4),
        number(5..7),
        number(8..10),
        number(11..13),
        number(14..16),
        number(17..19),
    ) else {
        return false;
    };
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let max_day = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    if day == 0 || day > max_day || hour > 23 || minute > 59 || second > 60 {
        return false;
    }
    let mut cursor = 19;
    if bytes.get(cursor) == Some(&b'.') {
        cursor += 1;
        let start = cursor;
        while bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
            cursor += 1;
        }
        if cursor == start {
            return false;
        }
    }
    match bytes.get(cursor) {
        Some(b'Z' | b'z') => cursor + 1 == bytes.len(),
        Some(b'+' | b'-') if cursor + 6 == bytes.len() && bytes.get(cursor + 3) == Some(&b':') => {
            matches!(
                (
                    number(cursor + 1..cursor + 3),
                    number(cursor + 4..cursor + 6)
                ),
                (Some(0..=23), Some(0..=59))
            )
        }
        _ => false,
    }
}

fn graphql_name(value: &str, label: &str) -> Result<(), ClientCompileError> {
    if !super::is_graphql_name(value) || value.starts_with("__") {
        Err(invalid(
            "client.manifest.graphql_name",
            format!("{label} `{value}` must be a valid GraphQL name"),
        ))
    } else {
        Ok(())
    }
}

fn unique_nonempty(values: &[String], label: &str) -> Result<(), ClientCompileError> {
    let mut seen = BTreeSet::new();
    for value in values {
        nonempty(value, label)?;
        if !seen.insert(value) {
            return Err(invalid(
                "client.manifest.duplicate_entry",
                format!("{label} entries must be unique"),
            ));
        }
    }
    Ok(())
}

fn nonempty(value: &str, label: &str) -> Result<(), ClientCompileError> {
    if value.trim().is_empty() {
        Err(invalid(
            "client.manifest.empty",
            format!("{label} must not be empty"),
        ))
    } else {
        Ok(())
    }
}

fn command_error(
    command: &ManifestCommand,
    code: &'static str,
    message: impl std::fmt::Display,
) -> ClientCompileError {
    invalid(
        code,
        format!("manifest command `{}` {message}", command.name),
    )
}

fn invalid(code: &'static str, message: impl Into<String>) -> ClientCompileError {
    ClientCompileError::manifest(code, message)
}
