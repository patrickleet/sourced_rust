use std::collections::{BTreeMap, BTreeSet};

use crate::client_compiler::manifest::{
    ManifestCommand, ManifestConfirmationKind, ManifestConsistencyKind, ManifestField,
    ManifestModel, ManifestProjector,
};
use crate::client_compiler::ClientCompileError;

use super::effects::{require_model, validate_expression, validate_key};
use super::support::{command_error, invalid, nonempty};
use super::CommandManifestValidation;

// Mirrors `projection_protocol::MAX_PROJECTION_EVIDENCE_BATCH_ITEMS`, which is
// not exported across the `distributed`/`distributed_cli` package boundary.
const MAX_CONFIRMATIONS: usize = 128;

pub(super) fn validate_confirmations(
    command: &ManifestCommand,
    models: &BTreeMap<String, ManifestModel>,
    projectors: &BTreeMap<&str, &ManifestProjector>,
    report: &mut CommandManifestValidation,
) -> Result<(), ClientCompileError> {
    let confirmations = command.extensions.confirmations.as_ref();
    match (command.extensions.consistency.kind, confirmations) {
        (ManifestConsistencyKind::Eventual, None) => {
            return Err(command_error(
                command,
                "client.manifest.command_confirmations",
                "eventual consistency requires confirmations",
            ));
        }
        (ManifestConsistencyKind::Atomic, Some(_)) => {
            return Err(command_error(
                command,
                "client.manifest.command_confirmations",
                "atomic consistency cannot declare asynchronous confirmations",
            ));
        }
        _ => {}
    }
    let Some(confirmations) = confirmations else {
        if command.extensions.consistency.kind == ManifestConsistencyKind::Succeeded {
            report
                .commands_requiring_revalidation
                .insert(command.name.clone());
        }
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

pub(super) fn validate_projector_inventory<'a>(
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
