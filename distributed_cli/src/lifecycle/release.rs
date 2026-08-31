use crate::contracts::ContractArtifactKind;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

use super::{
    digest_bytes, validate_portable_path, GenerationManifest, LifecycleError, LifecycleGraph,
};

pub const RELEASE_MANIFEST_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseMember {
    pub node_id: String,
    pub kind: ContractArtifactKind,
    pub path: String,
    pub identity: String,
    pub receipt_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseManifest {
    pub schema_version: u32,
    pub application: String,
    pub source_identity: String,
    pub lifecycle_graph_id: String,
    pub generation_id: String,
    pub members: Vec<ReleaseMember>,
    pub release_id: String,
}

impl ReleaseManifest {
    pub fn new(
        graph: &LifecycleGraph,
        generation: &GenerationManifest,
    ) -> Result<Self, LifecycleError> {
        generation.validate_against(graph)?;
        let mut members = Vec::new();
        for (node_id, receipt) in &generation.receipts {
            for (path, identity) in &receipt.output_identities {
                members.push(ReleaseMember {
                    node_id: node_id.clone(),
                    kind: receipt.kind,
                    path: path.clone(),
                    identity: identity.clone(),
                    receipt_id: receipt.receipt_id.clone(),
                });
            }
        }
        let mut manifest = Self {
            schema_version: RELEASE_MANIFEST_SCHEMA_VERSION,
            application: generation.application.clone(),
            source_identity: generation.source_identity.clone(),
            lifecycle_graph_id: generation.lifecycle_graph_id.clone(),
            generation_id: generation.generation_id.clone(),
            members,
            release_id: String::new(),
        };
        manifest.validate_against(graph, generation)?;
        manifest.release_id = manifest.expected_id()?;
        manifest.validate_against(graph, generation)?;
        Ok(manifest)
    }

    pub fn validate_against(
        &self,
        graph: &LifecycleGraph,
        generation: &GenerationManifest,
    ) -> Result<(), LifecycleError> {
        generation.validate_against(graph)?;
        if self.schema_version != RELEASE_MANIFEST_SCHEMA_VERSION
            || self.application != generation.application
            || self.source_identity != generation.source_identity
            || self.lifecycle_graph_id != generation.lifecycle_graph_id
            || self.generation_id != generation.generation_id
        {
            return Err(LifecycleError::new(
                "release manifest does not match its generation",
            ));
        }
        let mut observed = BTreeSet::new();
        let expected = generation
            .receipts
            .iter()
            .flat_map(|(node_id, receipt)| {
                receipt
                    .output_identities
                    .iter()
                    .map(move |(path, identity)| {
                        (
                            node_id.clone(),
                            receipt.kind,
                            path.clone(),
                            identity.clone(),
                            receipt.receipt_id.clone(),
                        )
                    })
            })
            .collect::<BTreeSet<_>>();
        for member in &self.members {
            validate_portable_path(&member.path, "release member")?;
            observed.insert((
                member.node_id.clone(),
                member.kind,
                member.path.clone(),
                member.identity.clone(),
                member.receipt_id.clone(),
            ));
        }
        if observed != expected || self.members.len() != expected.len() {
            return Err(LifecycleError::new(
                "release membership differs from the complete generation outputs",
            ));
        }
        if !self.release_id.is_empty() && self.release_id != self.expected_id()? {
            return Err(LifecycleError::new("release manifest identity is stale"));
        }
        Ok(())
    }

    pub fn canonical_bytes(
        &self,
        graph: &LifecycleGraph,
        generation: &GenerationManifest,
    ) -> Result<Vec<u8>, LifecycleError> {
        self.validate_against(graph, generation)?;
        serde_json::to_vec(self).map_err(|error| LifecycleError::new(error.to_string()))
    }

    fn expected_id(&self) -> Result<String, LifecycleError> {
        #[derive(Serialize)]
        struct IdentityView<'a> {
            schema_version: u32,
            application: &'a str,
            source_identity: &'a str,
            lifecycle_graph_id: &'a str,
            generation_id: &'a str,
            members: &'a [ReleaseMember],
        }
        let bytes = serde_json::to_vec(&IdentityView {
            schema_version: self.schema_version,
            application: &self.application,
            source_identity: &self.source_identity,
            lifecycle_graph_id: &self.lifecycle_graph_id,
            generation_id: &self.generation_id,
            members: &self.members,
        })
        .map_err(|error| LifecycleError::new(error.to_string()))?;
        Ok(digest_bytes(&bytes))
    }
}
