use super::artifact::canonical_digest;
use super::diagnostic::is_secret_like;
use super::{
    ArtifactIdentity, ArtifactPredecessor, ArtifactProvenance, ContractArtifactKind,
    ContractCheckResult, ContractDiagnostic, ContractDiagnosticCode, EnvironmentPolicyReference,
};
use serde::de::Error as DeError;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

/// Version of the repository catalog wire format.
pub const CONTRACT_CATALOG_SCHEMA_VERSION: u32 = 1;
/// Version of the declarative client inventory wire format.
pub const CLIENT_DECLARATION_SCHEMA_VERSION: u32 = 1;
/// Maximum accepted catalog or client-inventory source size.
pub const MAX_CATALOG_BYTES: usize = 1024 * 1024;
/// Maximum number of entries in one catalog.
pub const MAX_CATALOG_ENTRIES: usize = 256;
/// Maximum number of physical files walked by one catalog validation.
pub const MAX_CATALOG_FILES: usize = 8_192;
/// Maximum matches permitted for one catalog source glob.
pub const MAX_CATALOG_GLOB_MATCHES: usize = 2_048;

const MAX_CATALOG_STRING_BYTES: usize = 4 * 1024;
const MAX_CATALOG_JSON_DEPTH: usize = 24;
const MAX_CLIENT_DECLARATIONS: usize = 64;
const MAX_CLIENT_DOCUMENTS: usize = 64;

/// A typed, deterministic catalog validation error.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContractError {
    code: ContractDiagnosticCode,
    message: String,
}

impl ContractError {
    pub(crate) fn new(code: ContractDiagnosticCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// Stable diagnostic classification for this error.
    pub fn code(&self) -> ContractDiagnosticCode {
        self.code
    }

    /// Safe explanatory message.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for ContractError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ContractError {}

/// A stable logical scope in the contract catalog.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ContractScope {
    /// Stable scope ID; it is not a filesystem path.
    pub id: String,
}

/// One reference-only artifact declaration.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ContractEntry {
    /// Stable catalog entry ID.
    #[serde(default)]
    pub id: String,
    /// Artifact kind owned by another semantic module.
    pub kind: ContractArtifactKind,
    /// Logical scope containing this artifact.
    pub scope: ContractScope,
    /// Authoritative semantic owner ID.
    pub owner: String,
    /// Canonical artifact identity or producer reference.
    pub identity: ArtifactIdentity,
    /// Source and generator provenance, without semantic payloads.
    pub provenance: ArtifactProvenance,
    /// Immediate predecessor identity, when this artifact has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub predecessor: Option<ArtifactPredecessor>,
    /// Derived output ID to catalog-relative path.
    #[serde(default)]
    pub outputs: BTreeMap<String, String>,
    /// Lifecycle phases that may consume this reference.
    #[serde(default)]
    pub lifecycle: BTreeSet<String>,
    /// Environment policy identity/name/reference for deployment artifacts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_policy: Option<EnvironmentPolicyReference>,
}

/// The repository-level catalog. Its map representation is canonical JSON.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ContractCatalog {
    /// Catalog schema version.
    pub schema_version: u32,
    /// Entries keyed by stable entry ID.
    #[serde(deserialize_with = "deserialize_entries")]
    pub entries: BTreeMap<String, ContractEntry>,
}

impl ContractCatalog {
    /// Parse and structurally validate catalog JSON without touching the filesystem.
    pub fn from_json_str(input: &str) -> Result<Self, ContractError> {
        let value = parse_json_document(input, "catalog")?;
        reject_unknown_artifact_kinds(&value)?;
        let catalog: Self = serde_json::from_value(value).map_err(|error| {
            ContractError::new(
                ContractDiagnosticCode::CatalogInvalid,
                format!("parse catalog JSON: {error}"),
            )
        })?;
        catalog.validate_structure()?;
        Ok(catalog)
    }

    /// Alias for [`Self::from_json_str`].
    pub fn parse(input: &str) -> Result<Self, ContractError> {
        Self::from_json_str(input)
    }

    /// Read, parse, and physically validate a catalog file.
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, ContractError> {
        let path = path.as_ref();
        let root = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let canonical_root = fs::canonicalize(root).map_err(|error| {
            ContractError::new(
                ContractDiagnosticCode::CatalogPath,
                format!("resolve catalog repository root: {error}"),
            )
        })?;
        let canonical_catalog = fs::canonicalize(path).map_err(|error| {
            ContractError::new(
                ContractDiagnosticCode::CatalogSymlinkEscape,
                format!("resolve catalog path: {error}"),
            )
        })?;
        if !canonical_catalog.starts_with(&canonical_root) {
            return Err(ContractError::new(
                ContractDiagnosticCode::CatalogSymlinkEscape,
                "catalog path resolves outside the repository root",
            ));
        }
        let metadata = fs::metadata(&canonical_catalog).map_err(|error| {
            ContractError::new(
                ContractDiagnosticCode::CatalogPath,
                format!("inspect catalog path: {error}"),
            )
        })?;
        if !metadata.is_file() {
            return Err(ContractError::new(
                ContractDiagnosticCode::CatalogSpecialFile,
                "catalog path is not a regular file",
            ));
        }
        let bytes = read_bounded_file(path, MAX_CATALOG_BYTES, "catalog")?;
        let input = std::str::from_utf8(&bytes).map_err(|_| {
            ContractError::new(
                ContractDiagnosticCode::CatalogInvalid,
                "catalog is not UTF-8",
            )
        })?;
        let catalog = Self::from_json_str(input)?;
        catalog.validate_paths(canonical_root)?;
        Ok(catalog)
    }

    /// Alias for [`Self::from_path`].
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ContractError> {
        Self::from_path(path)
    }

    /// Serialize sorted catalog maps/sets as canonical bytes.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, ContractError> {
        serde_json::to_vec(self).map_err(|error| {
            ContractError::new(
                ContractDiagnosticCode::CatalogInvalid,
                format!("serialize canonical catalog: {error}"),
            )
        })
    }

    /// Validate all declared paths against a repository root without writing.
    pub fn validate_paths(&self, root: impl AsRef<Path>) -> Result<(), ContractError> {
        self.validate_structure()?;
        let root = fs::canonicalize(root.as_ref()).map_err(|error| {
            ContractError::new(
                ContractDiagnosticCode::CatalogPath,
                format!("resolve repository root: {error}"),
            )
        })?;
        let metadata = fs::metadata(&root).map_err(|error| {
            ContractError::new(
                ContractDiagnosticCode::CatalogPath,
                format!("inspect repository root: {error}"),
            )
        })?;
        if !metadata.is_dir() {
            return Err(ContractError::new(
                ContractDiagnosticCode::CatalogPath,
                "repository root is not a directory",
            ));
        }

        let mut walker = PhysicalPathWalker::new(root);
        for entry in self.entries.values() {
            for source in &entry.provenance.sources {
                walker.resolve_declared_path(
                    source,
                    entry.provenance.glob_limit,
                    "catalog source",
                )?;
            }
            for output in entry.outputs.values() {
                if contains_glob(output) {
                    return Err(ContractError::new(
                        ContractDiagnosticCode::CatalogUnboundedGlob,
                        format!("output path `{output}` may not contain a glob"),
                    ));
                }
                walker.resolve_declared_path(output, None, "catalog output")?;
            }
        }
        Ok(())
    }

    /// Validate and return a pure aggregate result for a read-only check.
    pub fn check(&self, root: impl AsRef<Path>) -> ContractCheckResult {
        let mut result = ContractCheckResult {
            catalog_identity: self
                .canonical_bytes()
                .ok()
                .map(|bytes| canonical_digest(&bytes)),
            artifacts: self
                .entries
                .iter()
                .map(|(id, entry)| (id.clone(), entry.identity.clone()))
                .collect(),
            diagnostics: BTreeSet::new(),
        };
        if let Err(error) = self.validate_paths(root) {
            result.push(diagnostic_for_error(&error));
        }
        result
    }

    /// Alias emphasizing that collection is pure and read-only.
    pub fn collect(&self, root: impl AsRef<Path>) -> ContractCheckResult {
        self.check(root)
    }

    fn validate_structure(&self) -> Result<(), ContractError> {
        if self.schema_version != CONTRACT_CATALOG_SCHEMA_VERSION {
            return Err(ContractError::new(
                ContractDiagnosticCode::CatalogInvalid,
                format!(
                    "unsupported catalog schema version {}; expected {}",
                    self.schema_version, CONTRACT_CATALOG_SCHEMA_VERSION
                ),
            ));
        }
        if self.entries.is_empty() {
            return Err(ContractError::new(
                ContractDiagnosticCode::CatalogInvalid,
                "catalog must declare at least one entry",
            ));
        }
        if self.entries.len() > MAX_CATALOG_ENTRIES {
            return Err(ContractError::new(
                ContractDiagnosticCode::CatalogInputLimit,
                format!(
                    "catalog declares {} entries; maximum is {MAX_CATALOG_ENTRIES}",
                    self.entries.len()
                ),
            ));
        }

        let mut scopes = BTreeMap::<&str, &str>::new();
        let mut owners = BTreeMap::<&str, &str>::new();
        let mut outputs = BTreeMap::<&str, (&str, &str)>::new();
        let mut output_paths = BTreeMap::<&str, (&str, &str)>::new();

        for (entry_id, entry) in &self.entries {
            validate_identifier(entry_id, "catalog entry ID")?;
            if entry.id != *entry_id {
                return Err(ContractError::new(
                    ContractDiagnosticCode::CatalogInvalid,
                    format!(
                        "entry key `{entry_id}` does not match entry ID `{}`",
                        entry.id
                    ),
                ));
            }
            validate_identifier(&entry.scope.id, "contract scope ID")?;
            validate_identifier(&entry.owner, "contract owner ID")?;
            if let Some(previous) = scopes.insert(&entry.scope.id, entry_id) {
                return Err(ContractError::new(
                    ContractDiagnosticCode::CatalogDuplicateScope,
                    format!(
                        "scope `{}` is declared by both `{previous}` and `{entry_id}`",
                        entry.scope.id
                    ),
                ));
            }
            if let Some(previous) = owners.insert(&entry.owner, entry_id) {
                return Err(ContractError::new(
                    ContractDiagnosticCode::CatalogDuplicateOwner,
                    format!(
                        "owner `{}` is declared by both `{previous}` and `{entry_id}`",
                        entry.owner
                    ),
                ));
            }
            if entry.identity.kind != entry.kind {
                return Err(ContractError::new(
                    ContractDiagnosticCode::ChainKindMismatch,
                    format!("entry `{entry_id}` identity kind does not match its artifact kind"),
                ));
            }
            validate_stable_value(&entry.identity.value, "artifact identity")?;
            if entry.provenance.sources.is_empty() {
                return Err(ContractError::new(
                    ContractDiagnosticCode::CatalogInvalid,
                    format!("entry `{entry_id}` must declare at least one source"),
                ));
            }
            validate_stable_value(&entry.provenance.generator, "generator identity")?;
            if let Some(revision) = &entry.provenance.source_revision {
                validate_stable_value(revision, "source revision")?;
            }
            if let Some(limit) = entry.provenance.glob_limit {
                if limit == 0 || limit > MAX_CATALOG_GLOB_MATCHES {
                    return Err(ContractError::new(
                        ContractDiagnosticCode::CatalogInputLimit,
                        format!(
                            "source glob limit {limit} is outside 1..={MAX_CATALOG_GLOB_MATCHES}"
                        ),
                    ));
                }
            }
            for source in &entry.provenance.sources {
                validate_catalog_path(source, true)?;
                if contains_glob(source) && entry.provenance.glob_limit.is_none() {
                    return Err(ContractError::new(
                        ContractDiagnosticCode::CatalogUnboundedGlob,
                        format!("source glob `{source}` has no finite match limit"),
                    ));
                }
            }
            if entry.outputs.is_empty() {
                return Err(ContractError::new(
                    ContractDiagnosticCode::CatalogInvalid,
                    format!("entry `{entry_id}` must declare at least one output"),
                ));
            }
            if entry.outputs.len() > MAX_CATALOG_ENTRIES {
                return Err(ContractError::new(
                    ContractDiagnosticCode::CatalogInputLimit,
                    format!("entry `{entry_id}` declares too many outputs"),
                ));
            }
            for (output_id, output_path) in &entry.outputs {
                validate_identifier(output_id, "catalog output ID")?;
                validate_catalog_path(output_path, false)?;
                if let Some((previous_entry, previous_path)) =
                    outputs.insert(output_id, (entry_id, output_path))
                {
                    return Err(ContractError::new(
                        ContractDiagnosticCode::CatalogDuplicateOutput,
                        format!(
                            "output ID `{output_id}` is declared by `{previous_entry}` ({previous_path}) and `{entry_id}` ({output_path})"
                        ),
                    ));
                }
                if let Some((previous_entry, previous_id)) =
                    output_paths.insert(output_path, (entry_id, output_id))
                {
                    return Err(ContractError::new(
                        ContractDiagnosticCode::CatalogDuplicateOutput,
                        format!(
                            "output path `{output_path}` is declared by `{previous_entry}` ({previous_id}) and `{entry_id}` ({output_id})"
                        ),
                    ));
                }
            }
            for lifecycle in &entry.lifecycle {
                validate_identifier(lifecycle, "lifecycle policy")?;
            }
            if let Some(policy) = &entry.environment_policy {
                validate_stable_value(&policy.identity, "environment policy identity")?;
                validate_identifier(&policy.name, "environment policy name")?;
                validate_stable_value(&policy.reference, "environment policy reference")?;
            }
            if let Some(predecessor) = &entry.predecessor {
                validate_identifier(&predecessor.entry_id, "predecessor entry ID")?;
                validate_stable_value(&predecessor.identity.value, "predecessor identity")?;
            }
        }

        self.validate_predecessors()
    }

    fn validate_predecessors(&self) -> Result<(), ContractError> {
        for (entry_id, entry) in &self.entries {
            let Some(predecessor) = &entry.predecessor else {
                continue;
            };
            let Some(predecessor_entry) = self.entries.get(&predecessor.entry_id) else {
                return Err(ContractError::new(
                    ContractDiagnosticCode::ChainMissingPredecessor,
                    format!(
                        "entry `{entry_id}` references missing predecessor `{}`",
                        predecessor.entry_id
                    ),
                ));
            };
            if predecessor.identity.kind != predecessor_entry.kind {
                return Err(ContractError::new(
                    ContractDiagnosticCode::ChainKindMismatch,
                    format!(
                        "entry `{entry_id}` predecessor `{}` declares kind {} but the entry is {}",
                        predecessor.entry_id, predecessor.identity.kind, predecessor_entry.kind
                    ),
                ));
            }
            if predecessor.identity.value != predecessor_entry.identity.value {
                return Err(ContractError::new(
                    ContractDiagnosticCode::ChainIdentityMismatch,
                    format!(
                        "entry `{entry_id}` predecessor `{}` has a stale identity",
                        predecessor.entry_id
                    ),
                ));
            }
        }

        for start in self.entries.keys() {
            let mut seen = BTreeSet::new();
            let mut current = start.as_str();
            while let Some(entry) = self.entries.get(current) {
                if !seen.insert(current.to_string()) {
                    return Err(ContractError::new(
                        ContractDiagnosticCode::ChainCycle,
                        format!("predecessor chain cycles at `{current}`"),
                    ));
                }
                let Some(predecessor) = &entry.predecessor else {
                    break;
                };
                current = &predecessor.entry_id;
            }
        }
        Ok(())
    }
}

/// One stable client declaration shared by Rust validation and the Vite config.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ClientDeclaration {
    /// Virtual module exposed to the application.
    pub module: String,
    /// Rust-declared application surface.
    pub surface: String,
    /// Co-located GraphQL files or bounded globs.
    pub documents: BTreeSet<String>,
    /// Compiler-owned output directory, relative to the UI root.
    pub output: String,
    /// Optional Rust surface export for non-default clients.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_entrypoint: Option<String>,
}

/// Versioned application-owned client inventory.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ClientInventory {
    /// Inventory schema version.
    pub schema_version: u32,
    /// Stable client declarations.
    pub clients: Vec<ClientDeclaration>,
}

impl ClientInventory {
    /// Parse and validate the shared client inventory without executing anything.
    pub fn from_json_str(input: &str) -> Result<Self, ContractError> {
        let value = parse_json_document(input, "client inventory")?;
        let inventory: Self = serde_json::from_value(value).map_err(|error| {
            ContractError::new(
                ContractDiagnosticCode::CatalogInvalid,
                format!("parse client inventory JSON: {error}"),
            )
        })?;
        inventory.validate()?;
        Ok(inventory)
    }

    /// Read and validate a client inventory file.
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, ContractError> {
        let bytes = read_bounded_file(path.as_ref(), MAX_CATALOG_BYTES, "client inventory")?;
        let input = std::str::from_utf8(&bytes).map_err(|_| {
            ContractError::new(
                ContractDiagnosticCode::CatalogInvalid,
                "client inventory is not UTF-8",
            )
        })?;
        Self::from_json_str(input)
    }

    /// Alias for [`Self::from_json_str`].
    pub fn parse(input: &str) -> Result<Self, ContractError> {
        Self::from_json_str(input)
    }

    /// Serialize a normalized client order and sorted document sets.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, ContractError> {
        let mut clients = self.clients.clone();
        clients.sort_by(|left, right| left.module.cmp(&right.module));
        serde_json::to_vec(&Self {
            schema_version: self.schema_version,
            clients,
        })
        .map_err(|error| {
            ContractError::new(
                ContractDiagnosticCode::CatalogInvalid,
                format!("serialize canonical client inventory: {error}"),
            )
        })
    }

    /// Validate the declaration schema and uniqueness constraints.
    pub fn validate(&self) -> Result<(), ContractError> {
        if self.schema_version != CLIENT_DECLARATION_SCHEMA_VERSION {
            return Err(ContractError::new(
                ContractDiagnosticCode::CatalogInvalid,
                format!(
                    "unsupported client inventory schema version {}; expected {}",
                    self.schema_version, CLIENT_DECLARATION_SCHEMA_VERSION
                ),
            ));
        }
        if self.clients.is_empty() || self.clients.len() > MAX_CLIENT_DECLARATIONS {
            return Err(ContractError::new(
                ContractDiagnosticCode::CatalogInputLimit,
                format!("client inventory must contain 1..={MAX_CLIENT_DECLARATIONS} declarations"),
            ));
        }
        let mut modules = BTreeSet::new();
        let mut surfaces = BTreeSet::new();
        let mut outputs = BTreeSet::new();
        for client in &self.clients {
            validate_client_module(&client.module)?;
            validate_identifier(&client.surface, "client surface")?;
            validate_catalog_path(&client.output, false)?;
            if contains_glob(&client.output) {
                return Err(ContractError::new(
                    ContractDiagnosticCode::CatalogUnboundedGlob,
                    format!("client output `{}` may not contain a glob", client.output),
                ));
            }
            if client.documents.is_empty() || client.documents.len() > MAX_CLIENT_DOCUMENTS {
                return Err(ContractError::new(
                    ContractDiagnosticCode::CatalogInputLimit,
                    format!(
                        "client `{}` must contain 1..={MAX_CLIENT_DOCUMENTS} documents",
                        client.module
                    ),
                ));
            }
            for document in &client.documents {
                validate_client_document(document)?;
            }
            if let Some(entrypoint) = &client.manifest_entrypoint {
                validate_entrypoint(entrypoint)?;
            }
            if !modules.insert(&client.module) {
                return Err(ContractError::new(
                    ContractDiagnosticCode::CatalogDuplicateScope,
                    format!(
                        "client module `{}` is declared more than once",
                        client.module
                    ),
                ));
            }
            if !surfaces.insert(&client.surface) {
                return Err(ContractError::new(
                    ContractDiagnosticCode::CatalogDuplicateScope,
                    format!(
                        "client surface `{}` is declared more than once",
                        client.surface
                    ),
                ));
            }
            if !outputs.insert(&client.output) {
                return Err(ContractError::new(
                    ContractDiagnosticCode::CatalogDuplicateOutput,
                    format!(
                        "client output `{}` is declared more than once",
                        client.output
                    ),
                ));
            }
        }
        Ok(())
    }
}

fn deserialize_entries<'de, D>(deserializer: D) -> Result<BTreeMap<String, ContractEntry>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum EntryContainer {
        Map(BTreeMap<String, ContractEntry>),
        List(Vec<ContractEntry>),
    }

    let container = EntryContainer::deserialize(deserializer)?;
    let mut entries = BTreeMap::new();
    match container {
        EntryContainer::Map(map) => {
            for (key, mut entry) in map {
                if entry.id.is_empty() {
                    entry.id = key.clone();
                }
                if entry.id != key {
                    return Err(D::Error::custom(format!(
                        "entry key `{key}` does not match entry ID `{}`",
                        entry.id
                    )));
                }
                entries.insert(key, entry);
            }
        }
        EntryContainer::List(list) => {
            for entry in list {
                if entry.id.is_empty() {
                    return Err(D::Error::custom("catalog list entries require an id"));
                }
                let id = entry.id.clone();
                if entries.insert(id.clone(), entry).is_some() {
                    return Err(D::Error::custom(format!("duplicate catalog entry `{id}`")));
                }
            }
        }
    }
    Ok(entries)
}

fn parse_json_document(input: &str, label: &str) -> Result<Value, ContractError> {
    if input.len() > MAX_CATALOG_BYTES {
        return Err(ContractError::new(
            ContractDiagnosticCode::CatalogInputLimit,
            format!(
                "{label} is {} bytes; maximum supported size is {MAX_CATALOG_BYTES}",
                input.len()
            ),
        ));
    }
    let value: Value = serde_json::from_str(input).map_err(|error| {
        ContractError::new(
            ContractDiagnosticCode::CatalogInvalid,
            format!("parse {label} JSON: {error}"),
        )
    })?;
    validate_json_value(&value, 0, "$", label)?;
    Ok(value)
}

fn validate_json_value(
    value: &Value,
    depth: usize,
    path: &str,
    label: &str,
) -> Result<(), ContractError> {
    if depth > MAX_CATALOG_JSON_DEPTH {
        return Err(ContractError::new(
            ContractDiagnosticCode::CatalogInputLimit,
            format!("{label} exceeds maximum JSON nesting depth"),
        ));
    }
    match value {
        Value::Object(object) => {
            for (key, child) in object {
                if is_forbidden_field(key) {
                    return Err(ContractError::new(
                        ContractDiagnosticCode::EnvironmentValue,
                        format!("{label} field `{key}` is not permitted in portable metadata"),
                    ));
                }
                validate_json_value(child, depth + 1, &format!("{path}.{key}"), label)?;
            }
        }
        Value::Array(array) => {
            if array.len() > MAX_CATALOG_ENTRIES * 32 {
                return Err(ContractError::new(
                    ContractDiagnosticCode::CatalogInputLimit,
                    format!("{label} array at {path} is too large"),
                ));
            }
            for (index, child) in array.iter().enumerate() {
                validate_json_value(child, depth + 1, &format!("{path}[{index}]"), label)?;
            }
        }
        Value::String(string) => {
            if string.len() > MAX_CATALOG_STRING_BYTES {
                return Err(ContractError::new(
                    ContractDiagnosticCode::CatalogInputLimit,
                    format!("{label} string at {path} is too large"),
                ));
            }
            if is_secret_like(string) {
                return Err(ContractError::new(
                    ContractDiagnosticCode::EnvironmentValue,
                    format!("{label} contains a credential-like value at {path}"),
                ));
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
    Ok(())
}

fn reject_unknown_artifact_kinds(value: &Value) -> Result<(), ContractError> {
    let Some(entries) = value.get("entries") else {
        return Ok(());
    };
    match entries {
        Value::Object(entries) => {
            for (entry_id, entry) in entries {
                check_kind(entry, entry_id)?;
            }
        }
        Value::Array(entries) => {
            for (index, entry) in entries.iter().enumerate() {
                check_kind(entry, &format!("entries[{index}]"))?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn check_kind(value: &Value, entry_id: &str) -> Result<(), ContractError> {
    let Some(kind) = value.get("kind") else {
        return Ok(());
    };
    let Some(kind) = kind.as_str() else {
        return Ok(());
    };
    if ContractArtifactKind::parse(kind).is_none() {
        return Err(ContractError::new(
            ContractDiagnosticCode::CatalogUnknownKind,
            format!("entry `{entry_id}` uses unknown artifact kind `{kind}`"),
        ));
    }
    Ok(())
}

fn is_forbidden_field(field: &str) -> bool {
    matches!(
        field.to_ascii_lowercase().as_str(),
        "credential"
            | "credentials"
            | "connection_string"
            | "connectionstring"
            | "password"
            | "token"
            | "secret"
            | "secrets"
            | "private_key"
            | "privatekey"
            | "header"
            | "headers"
            | "environment"
            | "environment_value"
            | "environment_values"
            | "raw_environment"
            | "raw_environment_values"
            | "env"
            | "key"
    )
}

fn validate_identifier(value: &str, label: &str) -> Result<(), ContractError> {
    if value.is_empty() || value.trim() != value {
        return Err(ContractError::new(
            ContractDiagnosticCode::CatalogInvalid,
            format!("{label} must be a non-empty trimmed value"),
        ));
    }
    if value.len() > MAX_CATALOG_STRING_BYTES
        || value.contains('\0')
        || value.contains('\\')
        || value.contains("..")
    {
        return Err(ContractError::new(
            ContractDiagnosticCode::CatalogInputLimit,
            format!("{label} is too long or not portable"),
        ));
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"._:/-".contains(&byte))
    {
        return Err(ContractError::new(
            ContractDiagnosticCode::CatalogInvalid,
            format!("{label} contains unsupported characters"),
        ));
    }
    Ok(())
}

fn validate_stable_value(value: &str, label: &str) -> Result<(), ContractError> {
    if value.is_empty() || value.trim() != value || value.len() > MAX_CATALOG_STRING_BYTES {
        return Err(ContractError::new(
            ContractDiagnosticCode::CatalogInvalid,
            format!("{label} must be a non-empty trimmed value"),
        ));
    }
    if is_secret_like(value)
        || value.contains('\0')
        || value.contains('\\')
        || value.starts_with('/')
        || value.starts_with('~')
        || value.contains("/Users/")
        || value.contains("/home/")
        || value.contains("\\Users\\")
        || value.contains("\\home\\")
        || looks_like_timestamp(value)
    {
        return Err(ContractError::new(
            ContractDiagnosticCode::EnvironmentValue,
            format!("{label} contains non-portable or sensitive material"),
        ));
    }
    Ok(())
}

fn looks_like_timestamp(value: &str) -> bool {
    value.len() >= 20
        && value.as_bytes().get(4) == Some(&b'-')
        && value.as_bytes().get(7) == Some(&b'-')
        && value.as_bytes().get(10) == Some(&b'T')
}

fn validate_catalog_path(value: &str, allow_glob: bool) -> Result<(), ContractError> {
    if value.is_empty()
        || value.trim() != value
        || value.contains('\0')
        || value.contains('\\')
        || value.starts_with('/')
        || value.starts_with('~')
        || (value.len() >= 2 && value.as_bytes()[1] == b':')
    {
        return Err(ContractError::new(
            ContractDiagnosticCode::CatalogPath,
            format!("catalog path `{value}` is absolute or not portable"),
        ));
    }
    for component in Path::new(value).components() {
        if !matches!(component, Component::Normal(_)) {
            return Err(ContractError::new(
                ContractDiagnosticCode::CatalogPath,
                format!("catalog path `{value}` contains parent or root traversal"),
            ));
        }
    }
    if !allow_glob && contains_glob(value) {
        return Err(ContractError::new(
            ContractDiagnosticCode::CatalogUnboundedGlob,
            format!("catalog path `{value}` contains an unbounded glob"),
        ));
    }
    Ok(())
}

fn validate_client_module(value: &str) -> Result<(), ContractError> {
    let mut segments = value.split('/');
    if segments.next() != Some("$distributed") {
        return Err(ContractError::new(
            ContractDiagnosticCode::CatalogInvalid,
            format!("client module `{value}` must start with $distributed"),
        ));
    }
    if segments.any(|segment| {
        segment.is_empty()
            || !segment
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
    }) {
        return Err(ContractError::new(
            ContractDiagnosticCode::CatalogInvalid,
            format!("client module `{value}` contains an unsupported segment"),
        ));
    }
    Ok(())
}

fn validate_client_document(value: &str) -> Result<(), ContractError> {
    validate_catalog_path(value, true)?;
    if !value.ends_with(".graphql") && !value.ends_with(".gql") {
        return Err(ContractError::new(
            ContractDiagnosticCode::CatalogInvalid,
            format!("client document `{value}` must end in .graphql or .gql"),
        ));
    }
    if value.contains("**")
        || value.starts_with('*')
        || value.starts_with('?')
        || value.starts_with('[')
        || value.starts_with('{')
    {
        return Err(ContractError::new(
            ContractDiagnosticCode::CatalogUnboundedGlob,
            format!("client document glob `{value}` is unbounded"),
        ));
    }
    Ok(())
}

fn validate_entrypoint(value: &str) -> Result<(), ContractError> {
    if value.is_empty()
        || value.split("::").any(|segment| {
            segment.is_empty()
                || !segment
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        })
    {
        return Err(ContractError::new(
            ContractDiagnosticCode::CatalogInvalid,
            format!("client manifest entrypoint `{value}` is not a Rust path"),
        ));
    }
    Ok(())
}

fn contains_glob(value: &str) -> bool {
    value
        .bytes()
        .any(|byte| matches!(byte, b'*' | b'?' | b'[' | b']' | b'{'))
}

fn read_bounded_file(path: &Path, limit: usize, label: &str) -> Result<Vec<u8>, ContractError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        ContractError::new(
            ContractDiagnosticCode::CatalogPath,
            format!("read {label}: {error}"),
        )
    })?;
    if metadata.file_type().is_symlink() {
        let canonical = fs::canonicalize(path).map_err(|error| {
            ContractError::new(
                ContractDiagnosticCode::CatalogSymlinkEscape,
                format!("resolve {label} symlink: {error}"),
            )
        })?;
        if !canonical.is_file() {
            return Err(ContractError::new(
                ContractDiagnosticCode::CatalogSpecialFile,
                format!("{label} symlink does not resolve to a regular file"),
            ));
        }
    } else if !metadata.is_file() {
        return Err(ContractError::new(
            ContractDiagnosticCode::CatalogSpecialFile,
            format!("{label} must be a regular file"),
        ));
    }
    let bytes = fs::read(path).map_err(|error| {
        ContractError::new(
            ContractDiagnosticCode::CatalogPath,
            format!("read {label}: {error}"),
        )
    })?;
    if bytes.len() > limit {
        return Err(ContractError::new(
            ContractDiagnosticCode::CatalogInputLimit,
            format!(
                "{label} is {} bytes; maximum supported size is {limit}",
                bytes.len()
            ),
        ));
    }
    Ok(bytes)
}

struct PhysicalPathWalker {
    root: PathBuf,
    files: BTreeSet<PathBuf>,
    directories: BTreeSet<PathBuf>,
}

impl PhysicalPathWalker {
    fn new(root: PathBuf) -> Self {
        Self {
            root,
            files: BTreeSet::new(),
            directories: BTreeSet::new(),
        }
    }

    fn resolve_declared_path(
        &mut self,
        declared: &str,
        glob_limit: Option<usize>,
        label: &str,
    ) -> Result<(), ContractError> {
        if contains_glob(declared) {
            let limit = glob_limit.ok_or_else(|| {
                ContractError::new(
                    ContractDiagnosticCode::CatalogUnboundedGlob,
                    format!("{label} glob `{declared}` has no finite match limit"),
                )
            })?;
            let pattern = self.root.join(declared);
            let pattern = pattern.to_str().ok_or_else(|| {
                ContractError::new(
                    ContractDiagnosticCode::CatalogPath,
                    format!("{label} glob is not valid UTF-8"),
                )
            })?;
            let matches = glob::glob(pattern).map_err(|error| {
                ContractError::new(
                    ContractDiagnosticCode::CatalogPath,
                    format!("expand {label} glob `{declared}`: {error}"),
                )
            })?;
            let mut count = 0;
            for matched in matches {
                let matched = matched.map_err(|error| {
                    ContractError::new(
                        ContractDiagnosticCode::CatalogPath,
                        format!("read {label} glob `{declared}`: {error}"),
                    )
                })?;
                count += 1;
                if count > limit {
                    return Err(ContractError::new(
                        ContractDiagnosticCode::CatalogInputLimit,
                        format!("{label} glob `{declared}` exceeds limit {limit}"),
                    ));
                }
                self.walk_target(&matched, declared, label)?;
            }
            if count == 0 {
                return Err(ContractError::new(
                    ContractDiagnosticCode::CatalogPath,
                    format!("{label} glob `{declared}` matched no entries"),
                ));
            }
            return Ok(());
        }
        self.walk_target(&self.root.join(declared), declared, label)
    }

    fn walk_target(
        &mut self,
        path: &Path,
        declared: &str,
        label: &str,
    ) -> Result<(), ContractError> {
        let canonical = fs::canonicalize(path).map_err(|error| {
            ContractError::new(
                ContractDiagnosticCode::CatalogPath,
                format!("resolve {label} `{declared}`: {error}"),
            )
        })?;
        if !canonical.starts_with(&self.root) {
            return Err(ContractError::new(
                ContractDiagnosticCode::CatalogSymlinkEscape,
                format!("{label} `{declared}` resolves outside the repository root"),
            ));
        }
        let metadata = fs::metadata(&canonical).map_err(|error| {
            ContractError::new(
                ContractDiagnosticCode::CatalogPath,
                format!("inspect {label} `{declared}`: {error}"),
            )
        })?;
        if metadata.is_file() {
            self.record_file(canonical, declared)
        } else if metadata.is_dir() {
            self.walk_directory(&canonical, declared)
        } else {
            Err(ContractError::new(
                ContractDiagnosticCode::CatalogSpecialFile,
                format!("{label} `{declared}` is not a regular file or directory"),
            ))
        }
    }

    fn walk_directory(&mut self, directory: &Path, declared: &str) -> Result<(), ContractError> {
        let directory = fs::canonicalize(directory).map_err(|error| {
            ContractError::new(
                ContractDiagnosticCode::CatalogPath,
                format!("resolve catalog directory `{declared}`: {error}"),
            )
        })?;
        if !self.directories.insert(directory.clone()) {
            return Ok(());
        }
        let mut entries = fs::read_dir(&directory)
            .map_err(|error| {
                ContractError::new(
                    ContractDiagnosticCode::CatalogPath,
                    format!("read catalog directory `{declared}`: {error}"),
                )
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| {
                ContractError::new(
                    ContractDiagnosticCode::CatalogPath,
                    format!("read catalog directory `{declared}`: {error}"),
                )
            })?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let symlink = fs::symlink_metadata(&path)
                .map_err(|error| {
                    ContractError::new(
                        ContractDiagnosticCode::CatalogPath,
                        format!("inspect catalog entry `{declared}`: {error}"),
                    )
                })?
                .file_type()
                .is_symlink();
            let canonical = fs::canonicalize(&path).map_err(|error| {
                ContractError::new(
                    if symlink {
                        ContractDiagnosticCode::CatalogSymlinkEscape
                    } else {
                        ContractDiagnosticCode::CatalogPath
                    },
                    format!("resolve catalog entry `{declared}`: {error}"),
                )
            })?;
            if !canonical.starts_with(&self.root) {
                return Err(ContractError::new(
                    ContractDiagnosticCode::CatalogSymlinkEscape,
                    format!("catalog entry under `{declared}` escapes the repository root"),
                ));
            }
            let metadata = fs::metadata(&canonical).map_err(|error| {
                ContractError::new(
                    ContractDiagnosticCode::CatalogPath,
                    format!("inspect catalog entry `{declared}`: {error}"),
                )
            })?;
            if metadata.is_dir() {
                self.walk_directory(&canonical, declared)?;
            } else if metadata.is_file() {
                self.record_file(canonical, declared)?;
            } else {
                return Err(ContractError::new(
                    ContractDiagnosticCode::CatalogSpecialFile,
                    format!("catalog entry under `{declared}` is special or unsupported"),
                ));
            }
        }
        Ok(())
    }

    fn record_file(&mut self, path: PathBuf, declared: &str) -> Result<(), ContractError> {
        if self.files.insert(path) && self.files.len() > MAX_CATALOG_FILES {
            return Err(ContractError::new(
                ContractDiagnosticCode::CatalogInputLimit,
                format!("catalog paths exceed {MAX_CATALOG_FILES} physical files at `{declared}`"),
            ));
        }
        Ok(())
    }
}

fn diagnostic_for_error(error: &ContractError) -> ContractDiagnostic {
    ContractDiagnostic::new(
        error.code(),
        None,
        None::<&str>,
        "contract-catalog",
        std::iter::empty::<&str>(),
        std::iter::empty::<&str>(),
        None::<&str>,
        None,
        None,
        None::<&str>,
        None,
        "inspect distributed.contracts.json",
    )
    .with_detail(error.message())
}
