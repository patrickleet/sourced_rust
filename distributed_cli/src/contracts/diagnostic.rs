use super::{ArtifactIdentity, ContractArtifactKind};
use serde::de::Deserializer;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// Stable diagnostic classifications shared by human and machine output.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub enum ContractDiagnosticCode {
    #[serde(rename = "CTL-CATALOG-INVALID")]
    CatalogInvalid,
    #[serde(rename = "CTL-CATALOG-LIMIT")]
    CatalogInputLimit,
    #[serde(rename = "CTL-CATALOG-PATH")]
    CatalogPath,
    #[serde(rename = "CTL-CATALOG-SYMLINK")]
    CatalogSymlinkEscape,
    #[serde(rename = "CTL-CATALOG-FILE")]
    CatalogSpecialFile,
    #[serde(rename = "CTL-CATALOG-KIND")]
    CatalogUnknownKind,
    #[serde(rename = "CTL-CATALOG-DUPLICATE-SCOPE")]
    CatalogDuplicateScope,
    #[serde(rename = "CTL-CATALOG-DUPLICATE-OWNER")]
    CatalogDuplicateOwner,
    #[serde(rename = "CTL-CATALOG-DUPLICATE-OUTPUT")]
    CatalogDuplicateOutput,
    #[serde(rename = "CTL-CATALOG-GLOB")]
    CatalogUnboundedGlob,
    #[serde(rename = "CTL-ENV-VALUE")]
    EnvironmentValue,
    #[serde(rename = "CTL-CHAIN-MISSING")]
    ChainMissingPredecessor,
    #[serde(rename = "CTL-CHAIN-KIND")]
    ChainKindMismatch,
    #[serde(rename = "CTL-CHAIN-IDENTITY")]
    ChainIdentityMismatch,
    #[serde(rename = "CTL-CHAIN-CYCLE")]
    ChainCycle,
    #[serde(rename = "CTL-MIG-HISTORY")]
    MigrationHistory,
    #[serde(rename = "CTL-MIG-INVENTORY")]
    MigrationInventory,
    #[serde(rename = "CTL-SCHEMA-DRIFT")]
    SchemaDrift,
    #[serde(rename = "CTL-MANIFEST-VERSION")]
    ManifestVersion,
    #[serde(rename = "CTL-PROTOCOL-DRIFT")]
    ProtocolDrift,
    #[serde(rename = "CTL-GEN-STALE")]
    GeneratedStale,
    #[serde(rename = "CTL-PROGRAM-INCOMPLETE")]
    ProgramIncomplete,
    #[serde(rename = "CTL-CHAIN-STALE")]
    ChainStale,
}

impl ContractDiagnosticCode {
    /// The stable code string used in logs and JSON output.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CatalogInvalid => "CTL-CATALOG-INVALID",
            Self::CatalogInputLimit => "CTL-CATALOG-LIMIT",
            Self::CatalogPath => "CTL-CATALOG-PATH",
            Self::CatalogSymlinkEscape => "CTL-CATALOG-SYMLINK",
            Self::CatalogSpecialFile => "CTL-CATALOG-FILE",
            Self::CatalogUnknownKind => "CTL-CATALOG-KIND",
            Self::CatalogDuplicateScope => "CTL-CATALOG-DUPLICATE-SCOPE",
            Self::CatalogDuplicateOwner => "CTL-CATALOG-DUPLICATE-OWNER",
            Self::CatalogDuplicateOutput => "CTL-CATALOG-DUPLICATE-OUTPUT",
            Self::CatalogUnboundedGlob => "CTL-CATALOG-GLOB",
            Self::EnvironmentValue => "CTL-ENV-VALUE",
            Self::ChainMissingPredecessor => "CTL-CHAIN-MISSING",
            Self::ChainKindMismatch => "CTL-CHAIN-KIND",
            Self::ChainIdentityMismatch => "CTL-CHAIN-IDENTITY",
            Self::ChainCycle => "CTL-CHAIN-CYCLE",
            Self::MigrationHistory => "CTL-MIG-HISTORY",
            Self::MigrationInventory => "CTL-MIG-INVENTORY",
            Self::SchemaDrift => "CTL-SCHEMA-DRIFT",
            Self::ManifestVersion => "CTL-MANIFEST-VERSION",
            Self::ProtocolDrift => "CTL-PROTOCOL-DRIFT",
            Self::GeneratedStale => "CTL-GEN-STALE",
            Self::ProgramIncomplete => "CTL-PROGRAM-INCOMPLETE",
            Self::ChainStale => "CTL-CHAIN-STALE",
        }
    }
}

impl fmt::Display for ContractDiagnosticCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A diagnostic value that is redacted at construction and deserialization.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct SafeDiagnosticValue(String);

impl SafeDiagnosticValue {
    /// Construct a value while removing credential-like material.
    pub fn new(value: impl AsRef<str>) -> Self {
        Self(redact_value(value.as_ref()))
    }

    /// Read the safely rendered value.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SafeDiagnosticValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for SafeDiagnosticValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Ok(Self::new(value))
    }
}

/// One stable, safely redacted contract diagnostic.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd)]
#[serde(deny_unknown_fields)]
pub struct ContractDiagnostic {
    /// Stable classification code.
    pub code: ContractDiagnosticCode,
    /// Referenced artifact kind, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_kind: Option<ContractArtifactKind>,
    /// Declared scope, when known.
    #[serde(
        default,
        deserialize_with = "deserialize_redacted_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub scope: Option<String>,
    /// Authoritative owner of the affected contract.
    #[serde(deserialize_with = "deserialize_redacted_string")]
    pub owner: String,
    /// Exact safe source paths involved in the diagnostic.
    #[serde(default, deserialize_with = "deserialize_redacted_set")]
    pub source_paths: BTreeSet<String>,
    /// Exact safe derived/output paths involved in the diagnostic.
    #[serde(default, deserialize_with = "deserialize_redacted_set")]
    pub derived_paths: BTreeSet<String>,
    /// Semantic path within an owner, if applicable.
    #[serde(
        default,
        deserialize_with = "deserialize_redacted_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub semantic_path: Option<String>,
    /// Safe expected value, if applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected: Option<SafeDiagnosticValue>,
    /// Safe observed value, if applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed: Option<SafeDiagnosticValue>,
    /// Required lifecycle classification, if applicable.
    #[serde(
        default,
        deserialize_with = "deserialize_redacted_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub required_classification: Option<String>,
    /// Whether merge-base evidence was available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge_base_available: Option<bool>,
    /// One safe repair or acceptance command.
    #[serde(deserialize_with = "deserialize_redacted_string")]
    pub repair_command: String,
    /// Stable, non-sensitive explanatory detail.
    #[serde(
        default,
        deserialize_with = "deserialize_redacted_string",
        skip_serializing_if = "String::is_empty"
    )]
    pub detail: String,
}

#[derive(Serialize)]
struct RedactedContractDiagnostic {
    code: ContractDiagnosticCode,
    #[serde(skip_serializing_if = "Option::is_none")]
    artifact_kind: Option<ContractArtifactKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    scope: Option<String>,
    owner: String,
    source_paths: BTreeSet<String>,
    derived_paths: BTreeSet<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    semantic_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    expected: Option<SafeDiagnosticValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    observed: Option<SafeDiagnosticValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    required_classification: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    merge_base_available: Option<bool>,
    repair_command: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    detail: String,
}

impl ContractDiagnostic {
    /// Build a diagnostic from safe facts. Values are redacted before storage.
    #[expect(clippy::too_many_arguments)]
    pub fn new<S, I, J, P, Q>(
        code: ContractDiagnosticCode,
        artifact_kind: Option<ContractArtifactKind>,
        scope: Option<S>,
        owner: impl AsRef<str>,
        source_paths: I,
        derived_paths: J,
        semantic_path: Option<P>,
        expected: Option<&str>,
        observed: Option<&str>,
        required_classification: Option<Q>,
        merge_base_available: Option<bool>,
        repair_command: impl AsRef<str>,
    ) -> Self
    where
        S: AsRef<str>,
        I: IntoIterator,
        I::Item: AsRef<str>,
        J: IntoIterator,
        J::Item: AsRef<str>,
        P: AsRef<str>,
        Q: AsRef<str>,
    {
        Self {
            code,
            artifact_kind,
            scope: scope.map(|value| redact_value(value.as_ref())),
            owner: redact_value(owner.as_ref()),
            source_paths: source_paths
                .into_iter()
                .map(|value| redact_value(value.as_ref()))
                .collect(),
            derived_paths: derived_paths
                .into_iter()
                .map(|value| redact_value(value.as_ref()))
                .collect(),
            semantic_path: semantic_path.map(|value| redact_value(value.as_ref())),
            expected: expected.map(SafeDiagnosticValue::new),
            observed: observed.map(SafeDiagnosticValue::new),
            required_classification: required_classification
                .map(|value| redact_value(value.as_ref())),
            merge_base_available,
            repair_command: redact_value(repair_command.as_ref()),
            detail: String::new(),
        }
    }

    /// Add a safe explanatory detail to an existing diagnostic.
    pub fn with_detail(mut self, detail: impl AsRef<str>) -> Self {
        self.detail = redact_value(detail.as_ref());
        self
    }

    /// Render the exact facts in the human-readable format.
    pub fn human(&self) -> String {
        self.redacted().human_unchecked()
    }

    fn redacted(&self) -> Self {
        Self {
            code: self.code,
            artifact_kind: self.artifact_kind,
            scope: self.scope.as_deref().map(redact_value),
            owner: redact_value(&self.owner),
            source_paths: self
                .source_paths
                .iter()
                .map(|value| redact_value(value))
                .collect(),
            derived_paths: self
                .derived_paths
                .iter()
                .map(|value| redact_value(value))
                .collect(),
            semantic_path: self.semantic_path.as_deref().map(redact_value),
            expected: self
                .expected
                .as_ref()
                .map(|value| SafeDiagnosticValue::new(value.as_str())),
            observed: self
                .observed
                .as_ref()
                .map(|value| SafeDiagnosticValue::new(value.as_str())),
            required_classification: self.required_classification.as_deref().map(redact_value),
            merge_base_available: self.merge_base_available,
            repair_command: redact_value(&self.repair_command),
            detail: redact_value(&self.detail),
        }
    }

    fn human_unchecked(&self) -> String {
        let mut output = self.code.to_string();
        if !self.detail.is_empty() {
            output.push_str(": ");
            output.push_str(&self.detail);
        }
        if let Some(kind) = self.artifact_kind {
            output.push_str(&format!(" [kind={kind}]"));
        }
        if let Some(scope) = &self.scope {
            output.push_str(&format!(" [scope={scope}]"));
        }
        output.push_str(&format!(" [owner={}]", self.owner));
        if !self.source_paths.is_empty() {
            output.push_str(&format!(" [source={}]", join_set(&self.source_paths)));
        }
        if !self.derived_paths.is_empty() {
            output.push_str(&format!(" [derived={}]", join_set(&self.derived_paths)));
        }
        if let Some(path) = &self.semantic_path {
            output.push_str(&format!(" [semantic_path={path}]"));
        }
        if let Some(expected) = &self.expected {
            output.push_str(&format!(" [expected={expected}]"));
        }
        if let Some(observed) = &self.observed {
            output.push_str(&format!(" [observed={observed}]"));
        }
        if let Some(classification) = &self.required_classification {
            output.push_str(&format!(" [classification={classification}]"));
        }
        if let Some(available) = self.merge_base_available {
            output.push_str(&format!(" [merge_base_available={available}]"));
        }
        output.push_str(&format!(" [repair={}]", self.repair_command));
        output
    }

    fn redacted_wire(&self) -> RedactedContractDiagnostic {
        let redacted = self.redacted();
        RedactedContractDiagnostic {
            code: redacted.code,
            artifact_kind: redacted.artifact_kind,
            scope: redacted.scope,
            owner: redacted.owner,
            source_paths: redacted.source_paths,
            derived_paths: redacted.derived_paths,
            semantic_path: redacted.semantic_path,
            expected: redacted.expected,
            observed: redacted.observed,
            required_classification: redacted.required_classification,
            merge_base_available: redacted.merge_base_available,
            repair_command: redacted.repair_command,
            detail: redacted.detail,
        }
    }

    /// Serialize the same safely redacted facts used by [`Self::human`].
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

impl Serialize for ContractDiagnostic {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.redacted_wire().serialize(serializer)
    }
}

impl fmt::Display for ContractDiagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.human())
    }
}

/// Aggregated read-only contract-check output.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ContractCheckResult {
    /// Identity of the canonical catalog, when serialization succeeded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_identity: Option<String>,
    /// Artifact identities collected by catalog entry ID.
    #[serde(default)]
    pub artifacts: BTreeMap<String, ArtifactIdentity>,
    /// Sorted independent diagnostics.
    #[serde(default)]
    pub diagnostics: BTreeSet<ContractDiagnostic>,
}

impl ContractCheckResult {
    /// Whether no diagnostics were collected.
    pub fn is_ok(&self) -> bool {
        self.diagnostics.is_empty()
    }

    /// Add one diagnostic while preserving deterministic order and uniqueness.
    pub fn push(&mut self, diagnostic: ContractDiagnostic) {
        self.diagnostics.insert(diagnostic);
    }

    /// Canonical JSON bytes for stable comparisons and evidence.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }

    /// Human output with diagnostics in stable order.
    pub fn human(&self) -> String {
        self.diagnostics
            .iter()
            .map(ContractDiagnostic::human)
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// JSON output for machine consumers.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

impl fmt::Display for ContractCheckResult {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.human())
    }
}

fn join_set(values: &BTreeSet<String>) -> String {
    values.iter().cloned().collect::<Vec<_>>().join(",")
}

fn deserialize_redacted_string<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    String::deserialize(deserializer).map(|value| redact_value(&value))
}

fn deserialize_redacted_option<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer).map(|value| value.map(|value| redact_value(&value)))
}

fn deserialize_redacted_set<'de, D>(deserializer: D) -> Result<BTreeSet<String>, D::Error>
where
    D: Deserializer<'de>,
{
    BTreeSet::<String>::deserialize(deserializer).map(|values| {
        values
            .into_iter()
            .map(|value| redact_value(&value))
            .collect()
    })
}

fn redact_value(value: &str) -> String {
    if is_secret_like(value) {
        "[REDACTED]".to_string()
    } else {
        value.to_string()
    }
}

pub(crate) fn is_secret_like(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.contains("postgres://")
        || lower.contains("postgresql://")
        || lower.contains("mysql://")
        || lower.contains("mongodb://")
        || lower.contains("bearer ")
        || lower.contains("password=")
        || lower.contains("token=")
        || lower.contains("secret=")
        || lower.contains("-----begin ")
}
