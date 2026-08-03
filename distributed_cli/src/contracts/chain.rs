//! Generic predecessor-staleness diagnostics for the approved artifact chain.
//!
//! This module does not reinterpret CMP/DPL payload semantics. It only compares
//! expected versus observed predecessor identities and reports owner/path.

use super::artifact::{ArtifactIdentity, ArtifactPredecessor, ContractArtifactKind};
use super::diagnostic::{ContractCheckResult, ContractDiagnostic, ContractDiagnosticCode};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// One observed predecessor link to validate.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ObservedPredecessor {
    /// Catalog entry or source path that owns this check.
    pub owner: String,
    /// Spec/source path for diagnostics.
    pub source_path: String,
    /// The predecessor the artifact claims.
    pub claimed: ArtifactPredecessor,
    /// The currently recorded identity for that predecessor entry, if any.
    pub observed: Option<ArtifactIdentity>,
}

/// Validate claimed predecessors against observed catalog identities (no writes).
pub fn check_predecessor_chain(
    observations: impl IntoIterator<Item = ObservedPredecessor>,
) -> ContractCheckResult {
    let mut diagnostics = BTreeSet::new();
    for observation in observations {
        match &observation.observed {
            None => {
                diagnostics.insert(
                    ContractDiagnostic::new(
                        ContractDiagnosticCode::ChainMissingPredecessor,
                        Some(observation.claimed.identity.kind),
                        None::<&str>,
                        &observation.owner,
                        [observation.source_path.as_str()],
                        std::iter::empty::<&str>(),
                        Some("predecessor.entry_id"),
                        Some(observation.claimed.entry_id.as_str()),
                        None,
                        Some("repair_catalog_predecessor"),
                        None,
                        "distributed contracts check --scope catalog",
                    )
                    .with_detail(format!(
                        "owner `{}` at `{}` references missing predecessor `{}`",
                        observation.owner, observation.source_path, observation.claimed.entry_id
                    )),
                );
            }
            Some(observed) if observed.kind != observation.claimed.identity.kind => {
                diagnostics.insert(
                    ContractDiagnostic::new(
                        ContractDiagnosticCode::ChainKindMismatch,
                        Some(observation.claimed.identity.kind),
                        None::<&str>,
                        &observation.owner,
                        [observation.source_path.as_str()],
                        std::iter::empty::<&str>(),
                        Some("predecessor.identity.kind"),
                        Some(observation.claimed.identity.kind.as_str()),
                        Some(observed.kind.as_str()),
                        Some("repair_predecessor_kind"),
                        None,
                        "distributed contracts check --scope catalog",
                    )
                    .with_detail(format!(
                        "owner `{}` at `{}` expected predecessor kind {} but observed {}",
                        observation.owner,
                        observation.source_path,
                        observation.claimed.identity.kind,
                        observed.kind
                    )),
                );
            }
            Some(observed) if observed.value != observation.claimed.identity.value => {
                diagnostics.insert(
                    ContractDiagnostic::new(
                        ContractDiagnosticCode::ChainIdentityMismatch,
                        Some(ContractArtifactKind::ApplicationManifest),
                        None::<&str>,
                        &observation.owner,
                        [observation.source_path.as_str()],
                        std::iter::empty::<&str>(),
                        Some("predecessor.identity.value"),
                        Some(observation.claimed.identity.value.as_str()),
                        Some(observed.value.as_str()),
                        Some("accept_application_manifest"),
                        None,
                        "distributed contracts accept --scope application_manifest",
                    )
                    .with_detail(format!(
                        "owner `{}` at `{}` has stale predecessor `{}`",
                        observation.owner, observation.source_path, observation.claimed.entry_id
                    )),
                );
            }
            Some(_) => {}
        }
    }
    ContractCheckResult {
        catalog_identity: None,
        artifacts: Default::default(),
        diagnostics,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::artifact::ArtifactPredecessor;

    #[test]
    fn stale_application_predecessor_reports_owner_path_and_identities() {
        let result = check_predecessor_chain([ObservedPredecessor {
            owner: "deployment-plan/todo".into(),
            source_path: "specs/application-composition/index#DeploymentPlan".into(),
            claimed: ArtifactPredecessor {
                entry_id: "app-manifest".into(),
                identity: ArtifactIdentity::new(
                    ContractArtifactKind::ApplicationManifest,
                    "sha256:expected",
                ),
            },
            observed: Some(ArtifactIdentity::new(
                ContractArtifactKind::ApplicationManifest,
                "sha256:observed-stale",
            )),
        }]);
        assert_eq!(result.diagnostics.len(), 1);
        let diagnostic = result.diagnostics.iter().next().unwrap();
        assert_eq!(
            diagnostic.code,
            ContractDiagnosticCode::ChainIdentityMismatch
        );
        assert_eq!(diagnostic.owner, "deployment-plan/todo");
        assert!(diagnostic
            .source_paths
            .iter()
            .any(|path| path.contains("application-composition")));
        assert_eq!(
            diagnostic.expected.as_ref().map(|v| v.as_str()),
            Some("sha256:expected")
        );
        assert_eq!(
            diagnostic.observed.as_ref().map(|v| v.as_str()),
            Some("sha256:observed-stale")
        );
    }

    #[test]
    fn missing_predecessor_is_deterministic_and_no_write() {
        let result = check_predecessor_chain([ObservedPredecessor {
            owner: "plan".into(),
            source_path: "plans/todo.json".into(),
            claimed: ArtifactPredecessor {
                entry_id: "missing".into(),
                identity: ArtifactIdentity::new(
                    ContractArtifactKind::ApplicationManifest,
                    "sha256:x",
                ),
            },
            observed: None,
        }]);
        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(
            result.diagnostics.iter().next().unwrap().code,
            ContractDiagnosticCode::ChainMissingPredecessor
        );
    }
}
