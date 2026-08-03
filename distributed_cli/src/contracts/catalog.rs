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
use std::io::Read;
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
/// Maximum number of unique physical directories walked by one validation.
pub const MAX_CATALOG_DIRECTORIES: usize = 2_048;
/// Maximum number of physical directory entries inspected by one validation.
pub const MAX_CATALOG_DIRECTORY_ENTRIES: usize = 4_096;
/// Maximum physical directory nesting depth accepted during discovery.
pub const MAX_CATALOG_DIRECTORY_DEPTH: usize = 64;
/// Maximum matches permitted for one catalog source glob.
pub const MAX_CATALOG_GLOB_MATCHES: usize = 2_048;

const MAX_CATALOG_STRING_BYTES: usize = 4 * 1024;
/// Maximum JSON nesting depth accepted before typed catalog deserialization.
pub const MAX_CATALOG_JSON_DEPTH: usize = 24;
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
        let bytes = read_bounded_file(&canonical_catalog, MAX_CATALOG_BYTES, "catalog")?;
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
        self.validate_structure()?;
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
        let mut result = ContractCheckResult::default();
        if let Err(error) = self.validate_structure() {
            result.push(diagnostic_for_error(&error));
            return result;
        }
        let canonical_catalog = match self.canonical_bytes() {
            Ok(bytes) => bytes,
            Err(error) => {
                result.push(diagnostic_for_error(&error));
                return result;
            }
        };
        result.catalog_identity = Some(canonical_digest(&canonical_catalog));
        result.artifacts = self
            .entries
            .iter()
            .map(|(id, entry)| (id.clone(), entry.identity.clone()))
            .collect();
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
                if contains_glob(source) {
                    validate_bounded_glob(source)?;
                    if entry.provenance.glob_limit.is_none() {
                        return Err(ContractError::new(
                            ContractDiagnosticCode::CatalogUnboundedGlob,
                            format!("source glob `{source}` has no finite match limit"),
                        ));
                    }
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
    #[serde(deserialize_with = "deserialize_documents")]
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
        self.validate()?;
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

fn deserialize_documents<'de, D>(deserializer: D) -> Result<BTreeSet<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let documents = Vec::<String>::deserialize(deserializer)?;
    let mut unique_documents = BTreeSet::new();
    for document in documents {
        if !unique_documents.insert(document.clone()) {
            return Err(D::Error::custom(format!(
                "duplicate client document `{document}`"
            )));
        }
    }
    Ok(unique_documents)
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
    if is_secret_like(value) {
        return Err(ContractError::new(
            ContractDiagnosticCode::EnvironmentValue,
            format!("{label} contains credential-like material"),
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
    if is_secret_like(value) {
        return Err(ContractError::new(
            ContractDiagnosticCode::EnvironmentValue,
            "catalog metadata contains credential-like path material",
        ));
    }
    if value.is_empty()
        || value.trim() != value
        || value.len() > MAX_CATALOG_STRING_BYTES
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
    if is_secret_like(value) {
        return Err(ContractError::new(
            ContractDiagnosticCode::EnvironmentValue,
            "client module contains credential-like material",
        ));
    }
    if value.len() > MAX_CATALOG_STRING_BYTES
        || value.trim() != value
        || value.contains('\0')
        || value.contains('\\')
        || value.contains("..")
    {
        return Err(ContractError::new(
            ContractDiagnosticCode::CatalogInvalid,
            format!("client module `{value}` is not portable"),
        ));
    }
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
                .as_bytes()
                .first()
                .is_some_and(|byte| byte.is_ascii_alphanumeric())
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
        || value.starts_with(']')
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
    if is_secret_like(value) {
        return Err(ContractError::new(
            ContractDiagnosticCode::EnvironmentValue,
            "client manifest entrypoint contains credential-like material",
        ));
    }
    if value.is_empty()
        || value.len() > MAX_CATALOG_STRING_BYTES
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

fn validate_bounded_glob(value: &str) -> Result<(), ContractError> {
    if value.contains("**") {
        return Err(ContractError::new(
            ContractDiagnosticCode::CatalogUnboundedGlob,
            format!("recursive catalog glob `{value}` is not bounded by a match limit"),
        ));
    }
    let components = value.split('/').collect::<Vec<_>>();
    let glob_components = components
        .iter()
        .enumerate()
        .filter(|(_, component)| contains_glob(component))
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    if glob_components.len() != 1 || glob_components[0] != components.len() - 1 {
        return Err(ContractError::new(
            ContractDiagnosticCode::CatalogUnboundedGlob,
            format!("catalog glob `{value}` must match entries in one bounded directory"),
        ));
    }
    Ok(())
}

fn glob_parent(value: &str) -> PathBuf {
    value
        .rsplit_once('/')
        .map_or_else(PathBuf::new, |(parent, _)| PathBuf::from(parent))
}

fn read_bounded_file(path: &Path, limit: usize, label: &str) -> Result<Vec<u8>, ContractError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        ContractError::new(
            ContractDiagnosticCode::CatalogPath,
            format!("read {label}: {error}"),
        )
    })?;
    if metadata.file_type().is_symlink() {
        return Err(ContractError::new(
            ContractDiagnosticCode::CatalogSymlinkEscape,
            format!("{label} must not be a symlink"),
        ));
    }
    if !metadata.is_file() {
        return Err(ContractError::new(
            ContractDiagnosticCode::CatalogSpecialFile,
            format!("{label} must be a regular file"),
        ));
    }
    let file_size = metadata.len();
    if file_size > limit as u64 {
        return Err(ContractError::new(
            ContractDiagnosticCode::CatalogInputLimit,
            format!("{label} is {file_size} bytes; maximum supported size is {limit}"),
        ));
    }
    let read_limit = limit.saturating_add(1);
    let file = fs::File::open(path).map_err(|error| {
        ContractError::new(
            ContractDiagnosticCode::CatalogPath,
            format!("read {label}: {error}"),
        )
    })?;
    let opened_metadata = file.metadata().map_err(|error| {
        ContractError::new(
            ContractDiagnosticCode::CatalogPath,
            format!("inspect opened {label}: {error}"),
        )
    })?;
    if !opened_metadata.is_file() {
        return Err(ContractError::new(
            ContractDiagnosticCode::CatalogSpecialFile,
            format!("opened {label} is not a regular file"),
        ));
    }
    let opened_size = opened_metadata.len();
    if opened_size > limit as u64 {
        return Err(ContractError::new(
            ContractDiagnosticCode::CatalogInputLimit,
            format!("opened {label} is {opened_size} bytes; maximum supported size is {limit}"),
        ));
    }
    let mut bytes = Vec::with_capacity(opened_size as usize);
    file.take(read_limit as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| {
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
    directory_entries: usize,
}

impl PhysicalPathWalker {
    fn new(root: PathBuf) -> Self {
        Self {
            root,
            files: BTreeSet::new(),
            directories: BTreeSet::new(),
            directory_entries: 0,
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
            validate_bounded_glob(declared)?;
            let final_component = declared.rsplit('/').next().ok_or_else(|| {
                ContractError::new(
                    ContractDiagnosticCode::CatalogPath,
                    format!("{label} glob `{declared}` has no final component"),
                )
            })?;
            let pattern = glob::Pattern::new(final_component).map_err(|error| {
                ContractError::new(
                    ContractDiagnosticCode::CatalogPath,
                    format!("parse {label} glob `{declared}`: {error}"),
                )
            })?;
            let matches = self.guard_glob_candidates(
                &glob_parent(declared),
                declared,
                label,
                &pattern,
                limit,
            )?;
            for matched in &matches {
                self.walk_canonical_target(matched, declared, label)?;
            }
            if matches.is_empty() {
                return Err(ContractError::new(
                    ContractDiagnosticCode::CatalogPath,
                    format!("{label} glob `{declared}` matched no entries"),
                ));
            }
            return Ok(());
        }
        self.walk_target(&self.root.join(declared), declared, label)
    }

    fn guard_glob_candidates(
        &mut self,
        parent: &Path,
        declared: &str,
        label: &str,
        pattern: &glob::Pattern,
        match_limit: usize,
    ) -> Result<Vec<PathBuf>, ContractError> {
        let candidate_directory = fs::canonicalize(self.root.join(parent)).map_err(|error| {
            ContractError::new(
                ContractDiagnosticCode::CatalogPath,
                format!("resolve {label} glob directory `{declared}`: {error}"),
            )
        })?;
        if !candidate_directory.starts_with(&self.root) {
            return Err(ContractError::new(
                ContractDiagnosticCode::CatalogSymlinkEscape,
                format!("{label} glob directory `{declared}` escapes the repository root"),
            ));
        }
        let metadata = fs::metadata(&candidate_directory).map_err(|error| {
            ContractError::new(
                ContractDiagnosticCode::CatalogPath,
                format!("inspect {label} glob directory `{declared}`: {error}"),
            )
        })?;
        if !metadata.is_dir() {
            return Err(ContractError::new(
                ContractDiagnosticCode::CatalogPath,
                format!("{label} glob directory `{declared}` is not a directory"),
            ));
        }
        let depth = candidate_directory
            .strip_prefix(&self.root)
            .map_err(|_| {
                ContractError::new(
                    ContractDiagnosticCode::CatalogSymlinkEscape,
                    format!("{label} glob directory `{declared}` escapes the repository root"),
                )
            })?
            .components()
            .count();
        if depth > MAX_CATALOG_DIRECTORY_DEPTH {
            return Err(ContractError::new(
                ContractDiagnosticCode::CatalogInputLimit,
                format!(
                    "catalog directory depth exceeds {MAX_CATALOG_DIRECTORY_DEPTH} at `{declared}`"
                ),
            ));
        }
        if self.directories.insert(candidate_directory.clone())
            && self.directories.len() > MAX_CATALOG_DIRECTORIES
        {
            return Err(ContractError::new(
                ContractDiagnosticCode::CatalogInputLimit,
                format!("catalog directories exceed {MAX_CATALOG_DIRECTORIES} at `{declared}`"),
            ));
        }
        let mut entries = Vec::new();
        for entry in fs::read_dir(&candidate_directory).map_err(|error| {
            ContractError::new(
                ContractDiagnosticCode::CatalogPath,
                format!("read {label} glob directory `{declared}`: {error}"),
            )
        })? {
            let entry = entry.map_err(|error| {
                ContractError::new(
                    ContractDiagnosticCode::CatalogPath,
                    format!("read {label} glob directory `{declared}`: {error}"),
                )
            })?;
            self.record_directory_entry(declared)?;
            entries.push(entry);
        }
        entries.sort_by_key(|entry| entry.file_name());

        let mut matches = Vec::new();
        for entry in entries {
            let file_name = entry.file_name();
            let Some(file_name) = file_name.to_str() else {
                continue;
            };
            if !pattern.matches(file_name) {
                continue;
            }
            let path = entry.path();
            let symlink = fs::symlink_metadata(&path)
                .map_err(|error| {
                    ContractError::new(
                        ContractDiagnosticCode::CatalogPath,
                        format!("inspect {label} glob candidate `{declared}`: {error}"),
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
                    format!("resolve {label} glob candidate `{declared}`: {error}"),
                )
            })?;
            if !canonical.starts_with(&self.root) {
                return Err(ContractError::new(
                    ContractDiagnosticCode::CatalogSymlinkEscape,
                    format!("{label} glob `{declared}` resolves outside the repository root"),
                ));
            }
            matches.push(canonical);
            if matches.len() > match_limit {
                return Err(ContractError::new(
                    ContractDiagnosticCode::CatalogInputLimit,
                    format!("{label} glob `{declared}` exceeds limit {match_limit}"),
                ));
            }
        }
        Ok(matches)
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
        self.walk_canonical_target(&canonical, declared, label)
    }

    fn walk_canonical_target(
        &mut self,
        canonical: &Path,
        declared: &str,
        label: &str,
    ) -> Result<(), ContractError> {
        let metadata = fs::metadata(canonical).map_err(|error| {
            ContractError::new(
                ContractDiagnosticCode::CatalogPath,
                format!("inspect {label} `{declared}`: {error}"),
            )
        })?;
        if metadata.is_file() {
            self.record_file(canonical.to_path_buf(), declared)
        } else if metadata.is_dir() {
            let depth = self.relative_depth(canonical, declared)?;
            self.walk_directory(canonical, declared, depth)
        } else {
            Err(ContractError::new(
                ContractDiagnosticCode::CatalogSpecialFile,
                format!("{label} `{declared}` is not a regular file or directory"),
            ))
        }
    }

    fn relative_depth(&self, path: &Path, declared: &str) -> Result<usize, ContractError> {
        path.strip_prefix(&self.root)
            .map(|relative| relative.components().count())
            .map_err(|_| {
                ContractError::new(
                    ContractDiagnosticCode::CatalogSymlinkEscape,
                    format!("catalog path `{declared}` escapes the repository root"),
                )
            })
    }

    fn walk_directory(
        &mut self,
        directory: &Path,
        declared: &str,
        initial_depth: usize,
    ) -> Result<(), ContractError> {
        let mut pending = vec![(directory.to_path_buf(), initial_depth)];
        while let Some((directory, depth)) = pending.pop() {
            if depth > MAX_CATALOG_DIRECTORY_DEPTH {
                return Err(ContractError::new(
                    ContractDiagnosticCode::CatalogInputLimit,
                    format!(
                        "catalog directory depth exceeds {MAX_CATALOG_DIRECTORY_DEPTH} at `{declared}`"
                    ),
                ));
            }
            if !self.directories.insert(directory.clone()) {
                continue;
            }
            if self.directories.len() > MAX_CATALOG_DIRECTORIES {
                return Err(ContractError::new(
                    ContractDiagnosticCode::CatalogInputLimit,
                    format!("catalog directories exceed {MAX_CATALOG_DIRECTORIES} at `{declared}`"),
                ));
            }

            let mut entries = Vec::new();
            for entry in fs::read_dir(&directory).map_err(|error| {
                ContractError::new(
                    ContractDiagnosticCode::CatalogPath,
                    format!("read catalog directory `{declared}`: {error}"),
                )
            })? {
                let entry = entry.map_err(|error| {
                    ContractError::new(
                        ContractDiagnosticCode::CatalogPath,
                        format!("read catalog directory `{declared}`: {error}"),
                    )
                })?;
                self.record_directory_entry(declared)?;
                entries.push(entry);
            }
            entries.sort_by_key(|entry| entry.file_name());

            let mut child_directories = Vec::new();
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
                    child_directories.push(canonical);
                } else if metadata.is_file() {
                    self.record_file(canonical, declared)?;
                } else {
                    return Err(ContractError::new(
                        ContractDiagnosticCode::CatalogSpecialFile,
                        format!("catalog entry under `{declared}` is special or unsupported"),
                    ));
                }
            }
            for child in child_directories.into_iter().rev() {
                pending.push((child, depth + 1));
            }
        }
        Ok(())
    }

    fn record_directory_entry(&mut self, declared: &str) -> Result<(), ContractError> {
        self.directory_entries += 1;
        if self.directory_entries > MAX_CATALOG_DIRECTORY_ENTRIES {
            return Err(ContractError::new(
                ContractDiagnosticCode::CatalogInputLimit,
                format!(
                    "catalog directory entries exceed {MAX_CATALOG_DIRECTORY_ENTRIES} at `{declared}`"
                ),
            ));
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
