//! Derived logical topology intent from a process mount selection.
//!
//! Physical worker routes, epochs, and subscriptions are framework-owned later
//! outputs. This module records only the logical intent inventory that
//! downstream runtime and renderers consume without reinterpreting mounts.

use serde::{Deserialize, Serialize};

use super::error::{ApplicationError, ApplicationResult};
use super::manifest::ApplicationManifest;
use super::mount::MountSelector;

/// One derived logical route or subscription intent.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TopologyIntent {
    CommandRoute {
        command_id: String,
        process_id: String,
        remote: bool,
    },
    ProjectionSubscription {
        projection_id: String,
        process_id: String,
        direct: bool,
        facts: Vec<String>,
    },
    SurfaceEndpoint {
        surface_id: String,
        process_id: String,
    },
    ExtensionHook {
        extension_id: String,
        process_id: String,
    },
}

/// Derive sorted topology intents for one process.
pub fn derive_topology(
    manifest: &ApplicationManifest,
    process_id: &str,
    mounts: &[MountSelector],
    remote_commands: bool,
) -> ApplicationResult<Vec<TopologyIntent>> {
    let mut intents = Vec::new();
    for mount in mounts {
        match mount {
            MountSelector::Command { id } => {
                if !manifest.commands.iter().any(|command| command.id == *id) {
                    return Err(ApplicationError::Missing {
                        kind: "command",
                        identity: id.clone(),
                    });
                }
                intents.push(TopologyIntent::CommandRoute {
                    command_id: id.clone(),
                    process_id: process_id.to_string(),
                    remote: remote_commands,
                });
            }
            MountSelector::Projector { id } => {
                let projection = manifest
                    .projections
                    .iter()
                    .find(|projection| projection.id == *id)
                    .ok_or_else(|| ApplicationError::Missing {
                        kind: "projection",
                        identity: id.clone(),
                    })?;
                let mut facts = projection.facts.clone();
                facts.sort();
                facts.dedup();
                intents.push(TopologyIntent::ProjectionSubscription {
                    projection_id: id.clone(),
                    process_id: process_id.to_string(),
                    direct: projection.direct,
                    facts,
                });
            }
            MountSelector::Surface { id } => {
                if !manifest.surfaces.iter().any(|surface| surface.id == *id) {
                    return Err(ApplicationError::Missing {
                        kind: "surface",
                        identity: id.clone(),
                    });
                }
                intents.push(TopologyIntent::SurfaceEndpoint {
                    surface_id: id.clone(),
                    process_id: process_id.to_string(),
                });
            }
            MountSelector::Extension { id } => {
                if !manifest
                    .extensions
                    .iter()
                    .any(|extension| extension.id == *id)
                {
                    return Err(ApplicationError::Missing {
                        kind: "extension",
                        identity: id.clone(),
                    });
                }
                intents.push(TopologyIntent::ExtensionHook {
                    extension_id: id.clone(),
                    process_id: process_id.to_string(),
                });
            }
        }
    }
    intents.sort();
    Ok(intents)
}
