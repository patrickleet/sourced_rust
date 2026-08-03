//! Dialect-aware migration inventory and immutable-history checks.
//!
//! The inventory is deliberately a small, explicit data model. It owns the
//! logical migration order and the source paths/checksums for each supported
//! dialect; SQLx remains the runtime owner of applying those bytes. The
//! history checker reads the comparison inventory and its SQL bytes from one
//! explicit Git revision, so changing a local checksum cannot hide a baseline
//! edit.

use super::diagnostic::is_secret_like;
use super::{
    ArtifactIdentity, ContractArtifactKind, ContractCheckResult, ContractDiagnostic,
    ContractDiagnosticCode, ContractError,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

/// The current migration inventory wire format.
pub const MIGRATION_INVENTORY_SCHEMA_VERSION: u32 = 1;
/// Repository-relative location of the inventory.
pub const MIGRATION_INVENTORY_PATH: &str = "migrations/inventory.json";
/// Maximum accepted inventory size.
pub const MAX_MIGRATION_INVENTORY_BYTES: usize = 1024 * 1024;
/// Maximum accepted SQL source size for one migration.
pub const MAX_MIGRATION_SQL_BYTES: usize = 4 * 1024 * 1024;
/// Maximum number of migrations in one inventory.
pub const MAX_MIGRATIONS: usize = 256;
/// Maximum nesting depth of JSON objects and arrays in an inventory.
pub const MAX_MIGRATION_JSON_DEPTH: usize = 24;
/// Maximum number of entries traversed beneath one dialect directory tree.
pub const MAX_MIGRATION_TOTAL_ENTRIES: usize = MAX_MIGRATIONS * 4;
/// Maximum number of direct entries beneath `migrations`.
pub const MAX_MIGRATION_TOP_LEVEL_ENTRIES: usize = 64;
/// The stable owner and scope used by migration diagnostics.
pub const MIGRATION_OWNER: &str = "distributed/migrations";
pub const MIGRATION_SCOPE: &str = "repository/migrations";

const DIALECT_DIRECTORY_LIMIT: usize = 4_096;

/// A SQL dialect with an explicit migration directory.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MigrationDialect {
    /// SQLite migrations.
    Sqlite,
    /// PostgreSQL migrations.
    Postgres,
}

impl MigrationDialect {
    /// All dialects required by the repository contract.
    pub const ALL: [Self; 2] = [Self::Sqlite, Self::Postgres];

    /// Stable inventory spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sqlite => "sqlite",
            Self::Postgres => "postgres",
        }
    }

    fn directory(self) -> &'static str {
        match self {
            Self::Sqlite => "migrations/sqlite",
            Self::Postgres => "migrations/postgres",
        }
    }
}

impl std::fmt::Display for MigrationDialect {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One dialect-specific SQL file declaration.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MigrationFile {
    /// Repository-relative path to the SQL file.
    pub path: String,
    /// Lowercase SHA-256 digest of the exact file bytes.
    pub sha256: String,
}

/// One logical migration and its required dialect implementations.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MigrationEntry {
    /// Consecutive logical migration version, starting at one.
    pub version: u64,
    /// Human-readable SQLx migration description.
    pub description: String,
    /// SQLite source declaration.
    pub sqlite: MigrationFile,
    /// PostgreSQL source declaration.
    pub postgres: MigrationFile,
}

impl MigrationEntry {
    /// Return the declaration for one supported dialect.
    pub fn file(&self, dialect: MigrationDialect) -> &MigrationFile {
        match dialect {
            MigrationDialect::Sqlite => &self.sqlite,
            MigrationDialect::Postgres => &self.postgres,
        }
    }
}

/// The single source of migration registration and history identity.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MigrationInventory {
    /// Inventory wire-format version.
    pub schema_version: u32,
    /// Migrations in their runtime application order.
    pub migrations: Vec<MigrationEntry>,
}

impl MigrationInventory {
    /// Parse and structurally validate inventory JSON without filesystem I/O.
    pub fn from_json_str(input: &str) -> Result<Self, ContractError> {
        if input.len() > MAX_MIGRATION_INVENTORY_BYTES {
            return Err(inventory_error(format!(
                "migration inventory is {} bytes; maximum supported size is {MAX_MIGRATION_INVENTORY_BYTES}",
                input.len()
            )));
        }
        validate_json_nesting(input)?;
        let value: Value = serde_json::from_str(input)
            .map_err(|error| inventory_error(format!("parse migration inventory JSON: {error}")))?;
        validate_json_value(&value, 1)?;
        let inventory: Self = serde_json::from_value(value)
            .map_err(|error| inventory_error(format!("parse migration inventory JSON: {error}")))?;
        inventory.validate_structure()?;
        Ok(inventory)
    }

    /// Alias for [`Self::from_json_str`].
    pub fn parse(input: &str) -> Result<Self, ContractError> {
        Self::from_json_str(input)
    }

    /// Read and fully validate an inventory at its conventional repository path.
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, ContractError> {
        let path = path.as_ref();
        let repository_root = inferred_repository_root(path);
        let canonical_root = canonical_repository_root(&repository_root)?;
        let relative = relative_path(&repository_root, path)?;
        reject_symlink_components(&canonical_root, &relative, "migration inventory")?;
        let bytes = read_bounded_file(path, MAX_MIGRATION_INVENTORY_BYTES, "migration inventory")?;
        let input = std::str::from_utf8(&bytes)
            .map_err(|_| inventory_error("migration inventory is not UTF-8".to_string()))?;
        let inventory = Self::from_json_str(input)?;
        inventory.validate_paths(&canonical_root)?;
        Ok(inventory)
    }

    /// Load and validate `migrations/inventory.json` beneath a repository root.
    pub fn from_repository_root(root: impl AsRef<Path>) -> Result<Self, ContractError> {
        Self::from_path(root.as_ref().join(MIGRATION_INVENTORY_PATH))
    }

    /// Serialize the validated inventory without changing its runtime order.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, ContractError> {
        self.validate_structure()?;
        serde_json::to_vec(self)
            .map_err(|error| inventory_error(format!("serialize migration inventory: {error}")))
    }

    /// Validate all declared files, checksums, dialect directories, and extras.
    pub fn validate_paths(&self, root: impl AsRef<Path>) -> Result<(), ContractError> {
        self.validate_structure()?;
        let root = canonical_repository_root(root.as_ref())?;
        let declared = self
            .migrations
            .iter()
            .flat_map(|migration| {
                MigrationDialect::ALL
                    .into_iter()
                    .map(move |dialect| (dialect, migration.file(dialect).path.clone()))
            })
            .collect::<BTreeSet<_>>();

        for migration in &self.migrations {
            for dialect in MigrationDialect::ALL {
                let declaration = migration.file(dialect);
                let bytes = read_migration_file(&root, declaration, dialect)?;
                let observed = sha256_hex(&bytes);
                if observed != declaration.sha256 {
                    return Err(inventory_error(format!(
                        "{} migration `{}` checksum mismatch: expected {}, observed {}",
                        dialect, declaration.path, declaration.sha256, observed
                    )));
                }
                if std::str::from_utf8(&bytes).is_err() {
                    return Err(inventory_error(format!(
                        "{} migration `{}` is not UTF-8 SQL",
                        dialect, declaration.path
                    )));
                }
            }
        }

        for dialect in MigrationDialect::ALL {
            let actual = collect_sql_files(&root, dialect)?;
            let declared_for_dialect = declared
                .iter()
                .filter(|(declared_dialect, _)| *declared_dialect == dialect)
                .cloned()
                .collect::<BTreeSet<_>>();
            if let Some(path) = actual.difference(&declared_for_dialect).next() {
                return Err(inventory_error(format!(
                    "extra {} migration file `{}` is not registered",
                    dialect, path.1
                )));
            }
        }
        validate_no_extra_dialect_directories(&root)?;
        Ok(())
    }

    /// Collect a read-only current-tree result with stable diagnostics.
    pub fn check(&self, root: impl AsRef<Path>) -> ContractCheckResult {
        let mut result = ContractCheckResult::default();
        let identity = match self.canonical_bytes() {
            Ok(identity) => identity,
            Err(error) => {
                result.push(diagnostic_for_error(&error));
                return result;
            }
        };
        result.catalog_identity = Some(super::artifact::canonical_digest(&identity));
        result.artifacts.insert(
            "migration-inventory".to_string(),
            ArtifactIdentity::from_canonical_bytes(
                ContractArtifactKind::MigrationInventory,
                &identity,
            ),
        );
        if let Err(error) = self.validate_paths(root) {
            result.push(diagnostic_for_error(&error));
        }
        result
    }

    /// Compare this current inventory against inventory and SQL bytes at a Git revision.
    pub fn check_history(
        &self,
        root: impl AsRef<Path>,
        base_revision: &str,
    ) -> MigrationHistoryCheck {
        let root = root.as_ref();
        let mut result = MigrationHistoryCheck {
            baseline: BaselineAvailability::Unavailable {
                revision: base_revision.to_string(),
                reason: "comparison has not started".to_string(),
            },
            diagnostics: BTreeSet::new(),
        };

        if let Err(error) = self.validate_paths(root) {
            result.push(diagnostic_for_error(&error));
        }

        let Ok(root) = canonical_repository_root(root) else {
            result.baseline = BaselineAvailability::Unavailable {
                revision: base_revision.to_string(),
                reason: "repository root is unavailable".to_string(),
            };
            result.push(unavailable_diagnostic(
                base_revision,
                "repository root is unavailable",
                false,
            ));
            return result;
        };
        if !valid_revision(base_revision) || !git_revision_exists(&root, base_revision) {
            result.baseline = BaselineAvailability::Unavailable {
                revision: base_revision.to_string(),
                reason: "explicit Git revision is unavailable".to_string(),
            };
            result.push(unavailable_diagnostic(
                base_revision,
                "immutable migration history evidence is unavailable for the explicit base revision",
                false,
            ));
            return result;
        }

        let baseline_bytes = match git_file(&root, base_revision, MIGRATION_INVENTORY_PATH) {
            Ok(bytes) => bytes,
            Err(reason) => {
                result.baseline = BaselineAvailability::Unavailable {
                    revision: base_revision.to_string(),
                    reason,
                };
                result.push(unavailable_diagnostic(
                    base_revision,
                    "the explicit Git revision has no readable migration inventory",
                    true,
                ));
                return result;
            }
        };
        let baseline_input = match std::str::from_utf8(&baseline_bytes) {
            Ok(input) => input,
            Err(_) => {
                result.baseline = BaselineAvailability::Unavailable {
                    revision: base_revision.to_string(),
                    reason: "baseline migration inventory is not UTF-8".to_string(),
                };
                result.push(unavailable_diagnostic(
                    base_revision,
                    "the explicit Git revision contains a non-UTF-8 migration inventory",
                    true,
                ));
                return result;
            }
        };
        let baseline = match Self::from_json_str(baseline_input) {
            Ok(inventory) => inventory,
            Err(error) => {
                result.baseline = BaselineAvailability::Unavailable {
                    revision: base_revision.to_string(),
                    reason: "baseline migration inventory is structurally invalid".to_string(),
                };
                result.push(
                    ContractDiagnostic::new(
                        ContractDiagnosticCode::MigrationHistory,
                        Some(ContractArtifactKind::MigrationInventory),
                        Some(MIGRATION_SCOPE),
                        MIGRATION_OWNER,
                        [MIGRATION_INVENTORY_PATH],
                        std::iter::empty::<&str>(),
                        None::<&str>,
                        Some(base_revision),
                        Some(error.message()),
                        Some("restore the baseline inventory and add a new migration"),
                        Some(true),
                        "restore the baseline inventory and add a new migration",
                    )
                    .with_detail("baseline migration inventory is structurally invalid"),
                );
                return result;
            }
        };

        result.baseline = BaselineAvailability::Available {
            revision: base_revision.to_string(),
        };
        let baseline_files = match load_baseline_files(&root, base_revision, &baseline) {
            Ok(files) => files,
            Err(reason) => {
                result.baseline = BaselineAvailability::Unavailable {
                    revision: base_revision.to_string(),
                    reason,
                };
                result.push(unavailable_diagnostic(
                    base_revision,
                    "the explicit Git revision has incomplete migration SQL evidence",
                    true,
                ));
                return result;
            }
        };
        compare_history(
            self,
            &baseline,
            &baseline_files,
            &root,
            base_revision,
            &mut result,
        );
        result
    }

    /// Alias emphasizing that the baseline comparison is read-only.
    pub fn compare_history(
        &self,
        root: impl AsRef<Path>,
        base_revision: &str,
    ) -> MigrationHistoryCheck {
        self.check_history(root, base_revision)
    }

    fn validate_structure(&self) -> Result<(), ContractError> {
        if self.schema_version != MIGRATION_INVENTORY_SCHEMA_VERSION {
            return Err(inventory_error(format!(
                "unsupported migration inventory schema version {}; expected {}",
                self.schema_version, MIGRATION_INVENTORY_SCHEMA_VERSION
            )));
        }
        if self.migrations.is_empty() || self.migrations.len() > MAX_MIGRATIONS {
            return Err(inventory_error(format!(
                "migration inventory must contain 1..={MAX_MIGRATIONS} migrations"
            )));
        }

        let mut paths = BTreeMap::<String, (u64, MigrationDialect)>::new();
        for (index, migration) in self.migrations.iter().enumerate() {
            let expected_version = (index + 1) as u64;
            if migration.version != expected_version {
                return Err(inventory_error(format!(
                    "migration versions must be ordered and consecutive: expected {expected_version}, observed {}",
                    migration.version
                )));
            }
            if migration.version > i64::MAX as u64 {
                return Err(inventory_error(format!(
                    "migration version {} exceeds SQLx's signed version range",
                    migration.version
                )));
            }
            validate_description(&migration.description, migration.version)?;
            for dialect in MigrationDialect::ALL {
                let file = migration.file(dialect);
                validate_migration_path(&file.path, dialect)?;
                validate_checksum(&file.sha256, &file.path)?;
                if let Some((previous_version, previous_dialect)) = paths.get(&file.path) {
                    return Err(inventory_error(format!(
                        "{} migration path `{}` is declared more than once ({} version {}, {} version {})",
                        dialect,
                        file.path,
                        previous_dialect,
                        previous_version,
                        dialect,
                        migration.version
                    )));
                }
                paths.insert(file.path.clone(), (migration.version, dialect));
            }
        }
        Ok(())
    }
}

/// The result of comparing an inventory with an explicit Git baseline.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MigrationHistoryCheck {
    /// Whether the explicit revision was available and readable.
    pub baseline: BaselineAvailability,
    /// Deterministically ordered history and current-tree diagnostics.
    pub diagnostics: BTreeSet<ContractDiagnostic>,
}

impl MigrationHistoryCheck {
    /// True only when baseline evidence exists and no diagnostic was emitted.
    pub fn is_verified(&self) -> bool {
        self.baseline.is_available() && self.diagnostics.is_empty()
    }

    /// Alias for [`Self::is_verified`]. Unavailable evidence is never success.
    pub fn is_ok(&self) -> bool {
        self.is_verified()
    }

    /// Whether the explicit comparison baseline could not be read.
    pub fn is_unavailable(&self) -> bool {
        !self.baseline.is_available()
    }

    /// Add one deterministic diagnostic.
    pub fn push(&mut self, diagnostic: ContractDiagnostic) {
        self.diagnostics.insert(diagnostic);
    }

    /// Human output shared with aggregate contract checks.
    pub fn human(&self) -> String {
        self.diagnostics
            .iter()
            .map(ContractDiagnostic::human)
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// JSON output shared with aggregate contract checks.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        let mut result = ContractCheckResult::default();
        for diagnostic in &self.diagnostics {
            result.push(diagnostic.clone());
        }
        result.to_json()
    }
}

/// Typed fact describing whether the comparison revision was available.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BaselineAvailability {
    /// Inventory and all declared SQL bytes were read from this revision.
    Available { revision: String },
    /// Comparison evidence was unavailable; this is not a successful check.
    Unavailable { revision: String, reason: String },
}

impl BaselineAvailability {
    /// Whether baseline inventory and SQL evidence was available.
    pub fn is_available(&self) -> bool {
        matches!(self, Self::Available { .. })
    }

    /// The requested revision, regardless of availability.
    pub fn revision(&self) -> &str {
        match self {
            Self::Available { revision } | Self::Unavailable { revision, .. } => revision,
        }
    }

    /// Unavailability explanation, if any.
    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Available { .. } => None,
            Self::Unavailable { reason, .. } => Some(reason),
        }
    }
}

/// Compare a repository's conventional inventory against an explicit Git revision.
pub fn check_migration_history(
    root: impl AsRef<Path>,
    base_revision: &str,
) -> MigrationHistoryCheck {
    let root = root.as_ref();
    match MigrationInventory::from_repository_root(root) {
        Ok(inventory) => inventory.check_history(root, base_revision),
        Err(error) => {
            let mut result = MigrationHistoryCheck {
                baseline: BaselineAvailability::Unavailable {
                    revision: base_revision.to_string(),
                    reason: "current migration inventory is invalid".to_string(),
                },
                diagnostics: BTreeSet::new(),
            };
            result.push(diagnostic_for_error(&error));
            result
        }
    }
}

/// Validate a repository's conventional inventory and SQL files.
pub fn check_migration_inventory(root: impl AsRef<Path>) -> ContractCheckResult {
    match MigrationInventory::from_repository_root(root.as_ref()) {
        Ok(inventory) => inventory.check(root),
        Err(error) => {
            let mut result = ContractCheckResult::default();
            result.push(diagnostic_for_error(&error));
            result
        }
    }
}

fn compare_history(
    current: &MigrationInventory,
    baseline: &MigrationInventory,
    baseline_files: &BTreeMap<(u64, MigrationDialect), Vec<u8>>,
    root: &Path,
    base_revision: &str,
    result: &mut MigrationHistoryCheck,
) {
    let current_by_version = current
        .migrations
        .iter()
        .map(|migration| (migration.version, migration))
        .collect::<BTreeMap<_, _>>();
    let current_last = current
        .migrations
        .last()
        .map_or(0, |migration| migration.version);
    let baseline_last = baseline
        .migrations
        .last()
        .map_or(0, |migration| migration.version);
    let next_version = current_last.max(baseline_last).saturating_add(1);

    let baseline_versions = baseline
        .migrations
        .iter()
        .map(|migration| migration.version)
        .collect::<Vec<_>>();
    let current_baseline_versions = current
        .migrations
        .iter()
        .filter(|migration| baseline_versions.contains(&migration.version))
        .map(|migration| migration.version)
        .collect::<Vec<_>>();
    if current_baseline_versions != baseline_versions {
        result.push(history_diagnostic(
            base_revision,
            next_version,
            baseline
                .migrations
                .iter()
                .flat_map(|migration| {
                    MigrationDialect::ALL.map(|dialect| migration.file(dialect).path.clone())
                })
                .collect::<Vec<_>>(),
            Some(&format!("versions={baseline_versions:?}")),
            Some(&format!("versions={current_baseline_versions:?}")),
            "baseline migration order or numbering changed",
        ));
    }

    for baseline_migration in &baseline.migrations {
        let Some(current_migration) = current_by_version.get(&baseline_migration.version) else {
            result.push(history_diagnostic(
                base_revision,
                next_version,
                MigrationDialect::ALL.map(|dialect| baseline_migration.file(dialect).path.clone()),
                Some(&format!(
                    "version {} is present",
                    baseline_migration.version
                )),
                Some("missing"),
                &format!(
                    "baseline migration {} was deleted; restore it and add migration {}",
                    baseline_migration.version, next_version
                ),
            ));
            continue;
        };

        if current_migration.description != baseline_migration.description {
            result.push(history_diagnostic(
                base_revision,
                next_version,
                MigrationDialect::ALL.map(|dialect| current_migration.file(dialect).path.clone()),
                Some(&baseline_migration.description),
                Some(&current_migration.description),
                &format!(
                    "baseline migration {} description changed; restore it and add migration {}",
                    baseline_migration.version, next_version
                ),
            ));
        }

        for dialect in MigrationDialect::ALL {
            let baseline_file = baseline_migration.file(dialect);
            let current_file = current_migration.file(dialect);
            if baseline_file.path != current_file.path {
                result.push(history_diagnostic(
                    base_revision,
                    next_version,
                    [baseline_file.path.clone(), current_file.path.clone()],
                    Some(&baseline_file.path),
                    Some(&current_file.path),
                    &format!(
                        "baseline migration {} {} path changed; restore it and add migration {}",
                        baseline_migration.version, dialect, next_version
                    ),
                ));
            }
            if baseline_file.sha256 != current_file.sha256 {
                result.push(history_diagnostic(
                    base_revision,
                    next_version,
                    [baseline_file.path.clone(), current_file.path.clone()],
                    Some(&baseline_file.sha256),
                    Some(&current_file.sha256),
                    &format!(
                        "baseline migration {} {} checksum changed; restore it and add migration {}",
                        baseline_migration.version, dialect, next_version
                    ),
                ));
            }
            let Some(baseline_bytes) = baseline_files.get(&(baseline_migration.version, dialect))
            else {
                continue;
            };
            let Ok(current_bytes) = read_migration_file(root, current_file, dialect) else {
                continue;
            };
            if baseline_bytes != &current_bytes {
                let baseline_hash = sha256_hex(baseline_bytes);
                let current_hash = sha256_hex(&current_bytes);
                result.push(history_diagnostic(
                    base_revision,
                    next_version,
                    [baseline_file.path.clone(), current_file.path.clone()],
                    Some(&baseline_hash),
                    Some(&current_hash),
                    &format!(
                        "baseline migration {} {} SQL bytes changed; restore it and add migration {}",
                        baseline_migration.version, dialect, next_version
                    ),
                ));
            }
        }
    }
}

fn load_baseline_files(
    root: &Path,
    revision: &str,
    inventory: &MigrationInventory,
) -> Result<BTreeMap<(u64, MigrationDialect), Vec<u8>>, String> {
    let mut files = BTreeMap::new();
    for migration in &inventory.migrations {
        for dialect in MigrationDialect::ALL {
            let declaration = migration.file(dialect);
            let bytes = git_file(root, revision, &declaration.path).map_err(|reason| {
                format!(
                    "unable to read baseline {} migration `{}`: {reason}",
                    dialect, declaration.path
                )
            })?;
            let observed = sha256_hex(&bytes);
            if observed != declaration.sha256 {
                return Err(format!(
                    "baseline {} migration `{}` checksum does not match its inventory",
                    dialect, declaration.path
                ));
            }
            if bytes.len() > MAX_MIGRATION_SQL_BYTES || std::str::from_utf8(&bytes).is_err() {
                return Err(format!(
                    "baseline {} migration `{}` is not bounded UTF-8 SQL",
                    dialect, declaration.path
                ));
            }
            files.insert((migration.version, dialect), bytes);
        }
    }
    Ok(files)
}

fn history_diagnostic<I, P>(
    base_revision: &str,
    next_version: u64,
    paths: I,
    expected: Option<&str>,
    observed: Option<&str>,
    detail: &str,
) -> ContractDiagnostic
where
    I: IntoIterator<Item = P>,
    P: AsRef<str>,
{
    let repair = format!(
        "restore the baseline migration from {base_revision} and add migration {next_version}"
    );
    let paths = paths
        .into_iter()
        .map(|path| declared_path_display(path.as_ref()))
        .collect::<Vec<_>>();
    ContractDiagnostic::new(
        ContractDiagnosticCode::MigrationHistory,
        Some(ContractArtifactKind::MigrationInventory),
        Some(MIGRATION_SCOPE),
        MIGRATION_OWNER,
        paths,
        [MIGRATION_INVENTORY_PATH],
        None::<&str>,
        expected,
        observed,
        Some(format!("add migration {next_version}")),
        Some(true),
        repair,
    )
    .with_detail(detail)
}

fn unavailable_diagnostic(
    base_revision: &str,
    detail: &str,
    merge_base_available: bool,
) -> ContractDiagnostic {
    ContractDiagnostic::new(
        ContractDiagnosticCode::MigrationHistory,
        Some(ContractArtifactKind::MigrationInventory),
        Some(MIGRATION_SCOPE),
        MIGRATION_OWNER,
        [MIGRATION_INVENTORY_PATH],
        std::iter::empty::<&str>(),
        None::<&str>,
        Some(base_revision),
        Some("unavailable"),
        Some("history evidence unavailable"),
        Some(merge_base_available),
        "rerun with `distributed contracts check --base <revision>`",
    )
    .with_detail(detail)
}

fn diagnostic_for_error(error: &ContractError) -> ContractDiagnostic {
    ContractDiagnostic::new(
        error.code(),
        Some(ContractArtifactKind::MigrationInventory),
        Some(MIGRATION_SCOPE),
        MIGRATION_OWNER,
        [MIGRATION_INVENTORY_PATH],
        std::iter::empty::<&str>(),
        None::<&str>,
        None,
        None,
        None::<&str>,
        None,
        "inspect migrations/inventory.json and declared SQL files",
    )
    .with_detail(error.message())
}

fn inventory_error(message: String) -> ContractError {
    ContractError::new(ContractDiagnosticCode::MigrationInventory, message)
}

fn validate_json_nesting(input: &str) -> Result<(), ContractError> {
    let mut depth = 0usize;
    let mut escaped = false;
    let mut in_string = false;

    for byte in input.bytes() {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }

        match byte {
            b'"' => in_string = true,
            b'{' | b'[' => {
                depth = depth.saturating_add(1);
                if depth > MAX_MIGRATION_JSON_DEPTH {
                    return Err(inventory_error(
                        "migration inventory exceeds maximum JSON nesting depth".to_string(),
                    ));
                }
            }
            b'}' | b']' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }

    Ok(())
}

fn validate_json_value(value: &Value, depth: usize) -> Result<(), ContractError> {
    if depth > MAX_MIGRATION_JSON_DEPTH {
        return Err(inventory_error(
            "migration inventory exceeds maximum JSON nesting depth".to_string(),
        ));
    }
    match value {
        Value::Object(object) => {
            for (key, child) in object {
                if is_secret_like(key) {
                    return Err(inventory_error(format!(
                        "migration inventory contains a credential-like field `{key}`"
                    )));
                }
                let child_depth = if child.is_object() || child.is_array() {
                    depth + 1
                } else {
                    depth
                };
                validate_json_value(child, child_depth)?;
            }
        }
        Value::Array(array) => {
            if array.len() > MAX_MIGRATIONS * 4 {
                return Err(inventory_error(
                    "migration inventory contains too many JSON array values".to_string(),
                ));
            }
            for child in array {
                let child_depth = if child.is_object() || child.is_array() {
                    depth + 1
                } else {
                    depth
                };
                validate_json_value(child, child_depth)?;
            }
        }
        Value::String(string) => {
            if string.len() > 4 * 1024 {
                return Err(inventory_error(
                    "migration inventory contains an oversized string".to_string(),
                ));
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
    Ok(())
}

fn validate_description(description: &str, version: u64) -> Result<(), ContractError> {
    if description.is_empty()
        || description.trim() != description
        || description.len() > 4 * 1024
        || description.contains('\0')
        || is_secret_like(description)
    {
        return Err(inventory_error(format!(
            "migration {version} description is empty, sensitive, or not portable"
        )));
    }
    Ok(())
}

fn validate_checksum(checksum: &str, path: &str) -> Result<(), ContractError> {
    if checksum.len() != 64
        || !checksum.bytes().all(|byte| byte.is_ascii_hexdigit())
        || checksum
            .chars()
            .any(|character| character.is_ascii_uppercase())
    {
        return Err(inventory_error(format!(
            "migration `{}` must declare one lowercase 64-character SHA-256 checksum",
            declared_path_display(path)
        )));
    }
    Ok(())
}

fn validate_migration_path(path: &str, dialect: MigrationDialect) -> Result<(), ContractError> {
    let display_path = declared_path_display(path);
    if path.is_empty()
        || path.trim() != path
        || path.len() > 4 * 1024
        || path.contains('\0')
        || path.contains('\\')
        || !path.ends_with(".sql")
    {
        return Err(inventory_error(format!(
            "{} migration path `{display_path}` is not a portable SQL path",
            dialect,
        )));
    }
    let path_value = Path::new(path);
    if path_value.is_absolute()
        || path_value
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        || !path_value.starts_with(dialect.directory())
    {
        return Err(inventory_error(format!(
            "{} migration path `{display_path}` must remain beneath `{}`",
            dialect,
            dialect.directory()
        )));
    }
    if is_secret_like(path) {
        return Err(inventory_error(format!(
            "{} migration path `{display_path}` contains sensitive material",
            dialect,
        )));
    }
    Ok(())
}

fn declared_path_display(path: &str) -> String {
    let path_value = Path::new(path);
    if path_value.is_absolute()
        || path.contains('\\')
        || path_value
            .components()
            .any(|component| matches!(component, Component::Prefix(_) | Component::RootDir))
    {
        "<outside-repository>".to_string()
    } else {
        path.to_string()
    }
}

fn canonical_repository_root(root: &Path) -> Result<PathBuf, ContractError> {
    let metadata = fs::symlink_metadata(root)
        .map_err(|error| inventory_error(format!("inspect migration repository root: {error}")))?;
    if metadata.file_type().is_symlink() {
        return Err(ContractError::new(
            ContractDiagnosticCode::CatalogSymlinkEscape,
            "migration repository root must not be a symlink".to_string(),
        ));
    }
    let root = fs::canonicalize(root)
        .map_err(|error| inventory_error(format!("resolve migration repository root: {error}")))?;
    let metadata = fs::metadata(&root)
        .map_err(|error| inventory_error(format!("inspect migration repository root: {error}")))?;
    if !metadata.is_dir() {
        return Err(inventory_error(
            "migration repository root is not a directory".to_string(),
        ));
    }
    Ok(root)
}

fn relative_path(root: &Path, path: &Path) -> Result<PathBuf, ContractError> {
    let current_directory = std::env::current_dir()
        .map_err(|error| inventory_error(format!("resolve current directory: {error}")))?;
    let absolute_root = if root.is_absolute() {
        root.to_path_buf()
    } else {
        current_directory.join(root)
    };
    let absolute_path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        current_directory.join(path)
    };
    absolute_path
        .strip_prefix(&absolute_root)
        .map(Path::to_path_buf)
        .map_err(|_| {
            inventory_error("migration inventory path escaped repository root".to_string())
        })
}

fn inferred_repository_root(path: &Path) -> PathBuf {
    if path.file_name().and_then(|name| name.to_str()) == Some("inventory.json")
        && path
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            == Some("migrations")
    {
        return path
            .parent()
            .and_then(Path::parent)
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    }
    path.parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

fn read_migration_file(
    root: &Path,
    declaration: &MigrationFile,
    dialect: MigrationDialect,
) -> Result<Vec<u8>, ContractError> {
    let path = root.join(&declaration.path);
    let display_path = declared_path_display(&declaration.path);
    let mut current = root.to_path_buf();
    let components = Path::new(&declaration.path)
        .components()
        .collect::<Vec<_>>();
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(component) = component else {
            return Err(inventory_error(format!(
                "{} migration path `{}` contains traversal",
                dialect, display_path
            )));
        };
        current.push(component);
        let metadata = fs::symlink_metadata(&current).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                inventory_error(format!(
                    "missing {} migration file `{}`",
                    dialect, display_path
                ))
            } else {
                inventory_error(format!(
                    "inspect {} migration file `{}`: {error}",
                    dialect, display_path
                ))
            }
        })?;
        if metadata.file_type().is_symlink() {
            return Err(ContractError::new(
                ContractDiagnosticCode::CatalogSymlinkEscape,
                format!(
                    "{} migration file `{}` must not be a symlink",
                    dialect, display_path
                ),
            ));
        }
        if index + 1 != components.len() && !metadata.is_dir() {
            return Err(inventory_error(format!(
                "{} migration path `{}` has a non-directory parent",
                dialect, display_path
            )));
        }
    }
    let metadata = fs::metadata(&path).map_err(|error| {
        inventory_error(format!(
            "inspect {} migration file `{}`: {error}",
            dialect, display_path
        ))
    })?;
    if !metadata.is_file() {
        return Err(
            ContractDiagnosticCode::CatalogSpecialFile.into_error(format!(
                "{} migration file `{}` is not regular",
                dialect, display_path
            )),
        );
    }
    let file = File::open(&path).map_err(|error| {
        inventory_error(format!(
            "open {} migration file `{}`: {error}",
            dialect, display_path
        ))
    })?;
    let opened_metadata = file.metadata().map_err(|error| {
        inventory_error(format!(
            "inspect opened {} migration file `{}`: {error}",
            dialect, display_path
        ))
    })?;
    if !opened_metadata.is_file() {
        return Err(
            ContractDiagnosticCode::CatalogSpecialFile.into_error(format!(
                "opened {} migration file `{}` is not regular",
                dialect, display_path
            )),
        );
    }
    if opened_metadata.len() > MAX_MIGRATION_SQL_BYTES as u64 {
        return Err(inventory_error(format!(
            "{} migration file `{}` exceeds {MAX_MIGRATION_SQL_BYTES} bytes",
            dialect, display_path
        )));
    }
    let mut bytes = Vec::with_capacity(opened_metadata.len() as usize);
    file.take(MAX_MIGRATION_SQL_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            inventory_error(format!(
                "read {} migration file `{}`: {error}",
                dialect, display_path
            ))
        })?;
    if bytes.len() > MAX_MIGRATION_SQL_BYTES {
        return Err(inventory_error(format!(
            "{} migration file `{}` exceeds {MAX_MIGRATION_SQL_BYTES} bytes",
            dialect, display_path
        )));
    }
    Ok(bytes)
}

fn relative_path_display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map(|relative| {
            relative
                .to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "/")
        })
        .unwrap_or_else(|_| "<outside-repository>".to_string())
}

fn collect_sql_files(
    root: &Path,
    dialect: MigrationDialect,
) -> Result<BTreeSet<(MigrationDialect, String)>, ContractError> {
    let directory = root.join(dialect.directory());
    let mut result = BTreeSet::new();
    let mut pending = vec![directory];
    let mut directories = 0;
    let mut entries_seen = 0usize;
    while let Some(directory) = pending.pop() {
        directories += 1;
        if directories > DIALECT_DIRECTORY_LIMIT {
            return Err(inventory_error(format!(
                "{} migration directory tree exceeds {DIALECT_DIRECTORY_LIMIT} directories",
                dialect
            )));
        }
        let directory_metadata = fs::symlink_metadata(&directory).map_err(|error| {
            inventory_error(format!(
                "inspect {} migration directory `{}`: {error}",
                dialect,
                relative_path_display(root, &directory)
            ))
        })?;
        if directory_metadata.file_type().is_symlink() {
            return Err(ContractError::new(
                ContractDiagnosticCode::CatalogSymlinkEscape,
                format!(
                    "{} migration directory `{}` must not be a symlink",
                    dialect,
                    relative_path_display(root, &directory)
                ),
            ));
        }
        if !directory_metadata.is_dir() {
            return Err(inventory_error(format!(
                "{} migration directory `{}` is not a directory",
                dialect,
                relative_path_display(root, &directory)
            )));
        }
        let entries = fs::read_dir(&directory).map_err(|error| {
            inventory_error(format!(
                "read {} migration directory `{}`: {error}",
                dialect,
                relative_path_display(root, &directory)
            ))
        })?;
        for entry in entries {
            entries_seen += 1;
            if entries_seen > MAX_MIGRATION_TOTAL_ENTRIES {
                return Err(inventory_error(format!(
                    "{} migration directory tree exceeds {MAX_MIGRATION_TOTAL_ENTRIES} entries",
                    dialect
                )));
            }
            let entry = entry.map_err(|error| {
                inventory_error(format!("read {dialect} migration directory entry: {error}"))
            })?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(|error| {
                inventory_error(format!(
                    "inspect {dialect} migration directory entry `{}`: {error}",
                    relative_path_display(root, &path)
                ))
            })?;
            if metadata.file_type().is_symlink() {
                return Err(ContractError::new(
                    ContractDiagnosticCode::CatalogSymlinkEscape,
                    format!(
                        "{} migration path `{}` must not be a symlink",
                        dialect,
                        relative_path_display(root, &path)
                    ),
                ));
            }
            if metadata.is_dir() {
                pending.push(path);
                continue;
            }
            if !metadata.is_file() {
                return Err(
                    ContractDiagnosticCode::CatalogSpecialFile.into_error(format!(
                        "{} migration path `{}` is not regular",
                        dialect,
                        relative_path_display(root, &path)
                    )),
                );
            }
            if path.extension().and_then(|extension| extension.to_str()) != Some("sql") {
                continue;
            }
            let relative = path
                .strip_prefix(root)
                .map_err(|_| inventory_error("migration path escaped repository root".to_string()))?
                .to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "/");
            result.insert((dialect, relative));
            if result.len() > MAX_MIGRATIONS * 2 {
                return Err(inventory_error(
                    "migration directory contains too many SQL files".to_string(),
                ));
            }
        }
    }
    Ok(result)
}

fn validate_no_extra_dialect_directories(root: &Path) -> Result<(), ContractError> {
    let migrations = root.join("migrations");
    let migrations_metadata = fs::symlink_metadata(&migrations)
        .map_err(|error| inventory_error(format!("inspect migrations directory: {error}")))?;
    if migrations_metadata.file_type().is_symlink() {
        return Err(ContractError::new(
            ContractDiagnosticCode::CatalogSymlinkEscape,
            "migrations directory must not be a symlink".to_string(),
        ));
    }
    if !migrations_metadata.is_dir() {
        return Err(inventory_error(
            "migrations path is not a directory".to_string(),
        ));
    }
    let entries = fs::read_dir(&migrations)
        .map_err(|error| inventory_error(format!("read migrations directory: {error}")))?;
    let mut entries_seen = 0usize;
    for entry in entries {
        entries_seen += 1;
        if entries_seen > MAX_MIGRATION_TOP_LEVEL_ENTRIES {
            return Err(inventory_error(format!(
                "migrations directory exceeds {MAX_MIGRATION_TOP_LEVEL_ENTRIES} entries"
            )));
        }
        let entry =
            entry.map_err(|error| inventory_error(format!("read migrations entry: {error}")))?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| inventory_error(format!("inspect migrations entry: {error}")))?;
        if metadata.file_type().is_symlink() {
            return Err(ContractError::new(
                ContractDiagnosticCode::CatalogSymlinkEscape,
                format!(
                    "migration path `{}` must not be a symlink",
                    relative_path_display(root, &path)
                ),
            ));
        }
        if !metadata.is_dir() {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        if !matches!(name, "sqlite" | "postgres") {
            return Err(inventory_error(format!(
                "unsupported migration dialect directory `migrations/{name}`"
            )));
        }
    }
    Ok(())
}

fn reject_symlink_components(
    root: &Path,
    relative: &Path,
    label: &str,
) -> Result<(), ContractError> {
    let mut current = root.to_path_buf();
    for component in relative.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(inventory_error(format!(
                    "{label} path `{}` contains parent traversal",
                    relative.display()
                )));
            }
            Component::Normal(part) => current.push(part),
            Component::Prefix(_) | Component::RootDir => {
                return Err(inventory_error(format!(
                    "{label} path `{}` is not relative to the repository root",
                    relative.display()
                )));
            }
        }
        let metadata = fs::symlink_metadata(&current).map_err(|error| {
            inventory_error(format!(
                "inspect {label} path `{}`: {error}",
                relative.display()
            ))
        })?;
        if metadata.file_type().is_symlink() {
            return Err(ContractError::new(
                ContractDiagnosticCode::CatalogSymlinkEscape,
                format!(
                    "{label} path `{}` must not traverse a symlink",
                    relative.display()
                ),
            ));
        }
    }
    Ok(())
}

fn read_bounded_file(path: &Path, limit: usize, label: &str) -> Result<Vec<u8>, ContractError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| inventory_error(format!("read {label}: {error}")))?;
    if metadata.file_type().is_symlink() {
        return Err(ContractError::new(
            ContractDiagnosticCode::CatalogSymlinkEscape,
            format!("{label} must not be a symlink"),
        ));
    }
    if !metadata.is_file() {
        return Err(ContractDiagnosticCode::CatalogSpecialFile
            .into_error(format!("{label} is not a regular file")));
    }
    if metadata.len() > limit as u64 {
        return Err(inventory_error(format!(
            "{label} is {} bytes; maximum supported size is {limit}",
            metadata.len()
        )));
    }
    let file =
        File::open(path).map_err(|error| inventory_error(format!("read {label}: {error}")))?;
    let opened_size = file
        .metadata()
        .map_err(|error| inventory_error(format!("inspect opened {label}: {error}")))?;
    if !opened_size.is_file() {
        return Err(ContractDiagnosticCode::CatalogSpecialFile
            .into_error(format!("opened {label} is not a regular file")));
    }
    let opened_size = opened_size.len();
    if opened_size > limit as u64 {
        return Err(inventory_error(format!(
            "opened {label} is {opened_size} bytes; maximum supported size is {limit}"
        )));
    }
    let mut bytes = Vec::with_capacity(opened_size as usize);
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| inventory_error(format!("read {label}: {error}")))?;
    if bytes.len() > limit {
        return Err(inventory_error(format!("{label} exceeds {limit} bytes")));
    }
    Ok(bytes)
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn valid_revision(revision: &str) -> bool {
    !revision.is_empty()
        && revision.trim() == revision
        && !revision.starts_with('-')
        && !revision.contains(':')
        && !revision.chars().any(char::is_control)
        && !revision.chars().any(char::is_whitespace)
}

fn git_revision_exists(root: &Path, revision: &str) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--verify", "--quiet", "--end-of-options"])
        .arg(format!("{revision}^{{commit}}"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn git_file(root: &Path, revision: &str, path: &str) -> Result<Vec<u8>, String> {
    let object = format!("{revision}:{path}");
    let limit = if path == MIGRATION_INVENTORY_PATH {
        MAX_MIGRATION_INVENTORY_BYTES
    } else {
        MAX_MIGRATION_SQL_BYTES
    };
    let mut child = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["show", "--no-ext-diff", "--format=", "--end-of-options"])
        .arg(object)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "could not invoke Git for the explicit revision".to_string())?;
    let mut stdout = child.stdout.take().ok_or_else(|| {
        terminate_child(&mut child);
        "Git did not provide a readable baseline stream".to_string()
    })?;
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 8 * 1024];
    loop {
        match stdout.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                if read > limit.saturating_sub(bytes.len()) {
                    terminate_child(&mut child);
                    return Err(format!("baseline file exceeds {limit} bytes"));
                }
                bytes.extend_from_slice(&buffer[..read]);
            }
            Err(_) => {
                terminate_child(&mut child);
                return Err("could not read the explicit Git baseline file".to_string());
            }
        }
    }
    drop(stdout);
    let status = child
        .wait()
        .map_err(|_| "could not wait for Git baseline file".to_string())?;
    if !status.success() {
        return Err("Git did not provide the requested baseline file".to_string());
    }
    Ok(bytes)
}

fn terminate_child(child: &mut std::process::Child) {
    let _ = child.kill();
    let _ = child.wait();
}

trait DiagnosticCodeError {
    fn into_error(self, message: String) -> ContractError;
}

impl DiagnosticCodeError for ContractDiagnosticCode {
    fn into_error(self, message: String) -> ContractError {
        ContractError::new(self, message)
    }
}
