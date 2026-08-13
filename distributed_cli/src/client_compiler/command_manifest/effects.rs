use std::collections::{BTreeMap, BTreeSet};

use crate::client_compiler::manifest::{
    ManifestCommand, ManifestCommandShape, ManifestEffect, ManifestEffectExpression,
    ManifestEffectField, ManifestEffectKey, ManifestEffectRelationship, ManifestField,
    ManifestInputDefault, ManifestModel, ManifestNormalization, ManifestProjectionPreviewSource,
    ManifestTypeField,
};
use crate::client_compiler::ClientCompileError;

use super::support::{command_error, constant_matches};
use super::CommandManifestValidation;

pub(super) fn validate_trusted_preset_inventory(
    command: &ManifestCommand,
) -> Result<(), ClientCompileError> {
    fn expression_names<'a>(expression: &'a ManifestEffectExpression, out: &mut BTreeSet<&'a str>) {
        if let ManifestEffectExpression::TrustedPreset { name } = expression {
            out.insert(name);
        }
    }
    fn key_names<'a>(key: &'a ManifestEffectKey, out: &mut BTreeSet<&'a str>) {
        for field in &key.fields {
            expression_names(&field.value, out);
        }
    }

    let mut referenced = BTreeSet::new();
    if let Some(effects) = &command.extensions.effects {
        for effect in &effects.operations {
            match effect {
                ManifestEffect::Upsert { key, fields, .. }
                | ManifestEffect::Patch { key, fields, .. } => {
                    key_names(key, &mut referenced);
                    for field in fields {
                        expression_names(&field.value, &mut referenced);
                    }
                }
                ManifestEffect::Delete { key, .. } => key_names(key, &mut referenced),
                ManifestEffect::Link { source, target, .. }
                | ManifestEffect::Unlink { source, target, .. } => {
                    key_names(source, &mut referenced);
                    key_names(target, &mut referenced);
                }
                ManifestEffect::InvalidateRelationship { source, .. } => {
                    key_names(source, &mut referenced);
                }
                ManifestEffect::InvalidateModel { .. } => {}
            }
        }
    }
    if let Some(confirmations) = &command.extensions.confirmations {
        for confirmation in &confirmations.expected {
            key_names(&confirmation.key, &mut referenced);
            if let Some(partition) = &confirmation.partition {
                expression_names(partition, &mut referenced);
            }
        }
    }
    if let Some(direct) = &command.extensions.direct_projection {
        if let Some(partition) = &direct.partition {
            expression_names(partition, &mut referenced);
        }
    }
    if let Some(projection) = &command.extensions.projection {
        for occurrence in &projection.preview_occurrences {
            for value in &occurrence.values {
                if let ManifestProjectionPreviewSource::TrustedPreset { name, .. } = &value.source {
                    referenced.insert(name);
                }
            }
        }
    }

    let declared = command
        .extensions
        .trusted_presets
        .iter()
        .map(|descriptor| descriptor.name.as_str())
        .collect::<BTreeSet<_>>();
    if declared != referenced {
        return Err(command_error(
            command,
            "client.manifest.trusted_preset_inventory",
            "trusted_presets must exactly describe every trusted preset expression",
        ));
    }
    Ok(())
}

pub(super) fn validate_defaults(
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

pub(super) fn validate_effect(
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

pub(super) fn validate_key(
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

pub(super) fn validate_expression(
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
        ManifestEffectExpression::TrustedPreset { name } => {
            let descriptor = command
                .extensions
                .trusted_presets
                .iter()
                .find(|descriptor| descriptor.name == *name)
                .ok_or_else(|| {
                    command_error(
                        command,
                        "client.manifest.effect_trusted_preset",
                        format!("uses undeclared trusted preset `{name}`"),
                    )
                })?;
            if descriptor.codec != expected.codec {
                return Err(command_error(
                    command,
                    "client.manifest.effect_trusted_preset",
                    format!(
                        "trusted preset `{name}` codec `{}` cannot populate `{}:{}` with codec `{}`",
                        descriptor.codec,
                        expected.name,
                        expected.scalar,
                        expected.codec
                    ),
                ));
            }
            Ok(())
        }
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

pub(super) fn require_model<'a>(
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
