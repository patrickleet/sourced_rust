use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;

/// The reference-level artifact vocabulary shared by lifecycle checks.
///
/// These variants identify semantic owners; they do not carry the payload of
/// the referenced artifact.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub enum ContractArtifactKind {
    /// Dialect-aware migration inventory and checksum registration.
    MigrationInventory,
    /// Role/application-visible surface or client manifest.
    SurfaceClientManifest,
    /// Compiler-owned generated client artifact tree.
    GeneratedClientTree,
    /// Framework-owned logical application manifest.
    ApplicationManifest,
    /// Framework-owned process-selection deployment plan.
    DeploymentPlan,
    /// Optional deployable UI program descriptor.
    ClientProgramDescriptor,
    /// Renderer-neutral resolved deployment.
    ResolvedDeployment,
}

impl ContractArtifactKind {
    /// Every kind accepted by the versioned catalog.
    pub const ALL: [Self; 7] = [
        Self::MigrationInventory,
        Self::SurfaceClientManifest,
        Self::GeneratedClientTree,
        Self::ApplicationManifest,
        Self::DeploymentPlan,
        Self::ClientProgramDescriptor,
        Self::ResolvedDeployment,
    ];

    /// The stable wire spelling for this kind.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MigrationInventory => "migration_inventory",
            Self::SurfaceClientManifest => "surface_client_manifest",
            Self::GeneratedClientTree => "generated_client_tree",
            Self::ApplicationManifest => "application_manifest",
            Self::DeploymentPlan => "deployment_plan",
            Self::ClientProgramDescriptor => "client_program_descriptor",
            Self::ResolvedDeployment => "resolved_deployment",
        }
    }

    /// Parse a stable wire spelling without accepting arbitrary enum values.
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == value)
    }
}

impl fmt::Display for ContractArtifactKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A portable content or owner-defined identity for one artifact.
///
/// `value` is intentionally opaque to this foundation. Producers may use a
/// content digest or another stable identity, but paths, timestamps, machine
/// locations, environment values, and secrets are not valid identity material.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactIdentity {
    /// The semantic kind whose identity is being represented.
    pub kind: ContractArtifactKind,
    /// Stable identity value, commonly `sha256:<lowercase-hex>`.
    pub value: String,
}

impl ArtifactIdentity {
    /// Construct an identity from an already validated stable value.
    pub fn new(kind: ContractArtifactKind, value: impl Into<String>) -> Self {
        Self {
            kind,
            value: value.into(),
        }
    }

    /// Create a SHA-256 identity from canonical bytes.
    pub fn from_canonical_bytes(kind: ContractArtifactKind, bytes: &[u8]) -> Self {
        Self::new(kind, canonical_digest(bytes))
    }
}

/// Source references and generator identity for an artifact.
///
/// `generator` is descriptive metadata only. The catalog never executes it
/// and never interprets it as a shell command.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactProvenance {
    /// Catalog-relative authoritative source files or bounded glob patterns.
    pub sources: std::collections::BTreeSet<String>,
    /// Stable generator identifier, not an executable command.
    pub generator: String,
    /// Optional source revision recorded by the producer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_revision: Option<String>,
    /// Maximum number of files a source glob may resolve to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub glob_limit: Option<usize>,
}

/// The immediate predecessor link in the lifecycle chain.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactPredecessor {
    /// Catalog entry ID of the predecessor.
    #[serde(alias = "entry")]
    pub entry_id: String,
    /// Identity observed when this artifact was produced.
    pub identity: ArtifactIdentity,
}

/// An environment-owned policy reference without policy values or secrets.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentPolicyReference {
    /// Immutable environment policy identity.
    pub identity: String,
    /// Human-stable policy name.
    pub name: String,
    /// Portable owner/reference identifier.
    pub reference: String,
}

fn hex_digest(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(crate) fn canonical_digest(bytes: &[u8]) -> String {
    format!("sha256:{}", hex_digest(bytes))
}
