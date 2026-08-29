use fs2::FileExt;
use glob::{glob_with, MatchOptions};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::contracts::ContractCatalog;

use super::{
    digest_bytes, validate_content_identity, validate_portable_path, validate_stable_value,
    ArtifactNodeReceipt, DistributedSourceIdentity, GenerationManifest, LifecycleConfig,
    LifecycleDevConfig, LifecycleError, LifecycleGraph, ReleaseManifest,
    LIFECYCLE_CONFIG_SCHEMA_VERSION,
};

pub const LIFECYCLE_BUILD_CONFIG_SCHEMA_VERSION: u32 = 1;
const MAX_EXECUTORS: usize = 256;
const MAX_EXECUTOR_ARGS: usize = 256;
const MAX_EXECUTOR_ARG_BYTES: usize = 16 * 1024;
const MAX_HASHED_FILES: usize = 8192;
const MAX_HASHED_FILE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_HASHED_TOTAL_BYTES: u64 = 256 * 1024 * 1024;
const MAX_HASH_DEPTH: usize = 64;
const MAX_LOCK_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LifecycleExecutor {
    /// Stable tool identity (for example a compiler version plus config digest).
    pub identity: String,
    /// Executable name or path. Commands are invoked directly, never by a shell.
    pub program: String,
    /// Argument vector. `{root}`, `{stage}`, and `{node}` are expanded per invocation.
    #[serde(default)]
    pub args: Vec<String>,
    /// Optional declared node output that receives the executor's stdout bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdout: Option<String>,
}

impl LifecycleExecutor {
    fn validate(&self, name: &str) -> Result<(), LifecycleError> {
        validate_stable_value(name, "lifecycle executor name")?;
        validate_content_identity(&self.identity, "lifecycle executor identity")?;
        validate_stable_value(&self.program, "lifecycle executor program")?;
        if self.args.len() > MAX_EXECUTOR_ARGS
            || self.args.iter().map(String::len).sum::<usize>() > MAX_EXECUTOR_ARG_BYTES
        {
            return Err(LifecycleError::new(format!(
                "executor `{name}` exceeds argument bounds"
            )));
        }
        if self.args.iter().any(|arg| arg.contains('\0')) {
            return Err(LifecycleError::new(format!(
                "executor `{name}` contains a NUL argument"
            )));
        }
        if let Some(stdout) = &self.stdout {
            validate_portable_path(stdout, "lifecycle executor stdout")?;
        }
        Ok(())
    }
}

/// Path/target-only lifecycle selection plus explicit native command adapters.
/// Artifact membership remains owned by the contract catalog.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LifecycleBuildConfig {
    pub schema_version: u32,
    pub application: String,
    pub source: DistributedSourceIdentity,
    pub roots: BTreeSet<String>,
    pub executors: BTreeMap<String, LifecycleExecutor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dev: Option<LifecycleDevConfig>,
}

impl LifecycleBuildConfig {
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, LifecycleError> {
        let metadata = fs::symlink_metadata(path.as_ref()).map_err(|error| {
            LifecycleError::new(format!(
                "failed to inspect lifecycle config `{}`: {error}",
                path.as_ref().display()
            ))
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(LifecycleError::new(
                "lifecycle config must be a regular non-symlink file",
            ));
        }
        if metadata.len() > 1024 * 1024 {
            return Err(LifecycleError::new("lifecycle config exceeds 1 MiB"));
        }
        let bytes = fs::read(path.as_ref()).map_err(|error| {
            LifecycleError::new(format!(
                "failed to read lifecycle config `{}`: {error}",
                path.as_ref().display()
            ))
        })?;
        let config: Self = serde_json::from_slice(&bytes).map_err(|error| {
            LifecycleError::new(format!(
                "failed to parse lifecycle config `{}`: {error}",
                path.as_ref().display()
            ))
        })?;
        config.validate()?;
        Ok(config)
    }

    pub fn lifecycle(&self) -> LifecycleConfig {
        LifecycleConfig {
            schema_version: LIFECYCLE_CONFIG_SCHEMA_VERSION,
            application: self.application.clone(),
            source: self.source.clone(),
            roots: self.roots.clone(),
        }
    }

    pub fn validate(&self) -> Result<(), LifecycleError> {
        if self.schema_version != LIFECYCLE_BUILD_CONFIG_SCHEMA_VERSION {
            return Err(LifecycleError::new(format!(
                "unsupported lifecycle build config schema version {}; expected {}",
                self.schema_version, LIFECYCLE_BUILD_CONFIG_SCHEMA_VERSION
            )));
        }
        self.lifecycle().validate()?;
        if self.executors.is_empty() || self.executors.len() > MAX_EXECUTORS {
            return Err(LifecycleError::new(format!(
                "lifecycle executors must contain 1..={MAX_EXECUTORS} entries"
            )));
        }
        for (name, executor) in &self.executors {
            executor.validate(name)?;
        }
        if let Some(dev) = &self.dev {
            dev.validate()?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct LifecycleBuildOptions {
    pub root: PathBuf,
    pub catalog: PathBuf,
    pub config: PathBuf,
    pub out: PathBuf,
    pub check: bool,
    pub lock_timeout: Duration,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BuildDrift {
    pub node_id: String,
    pub output: String,
    pub built_identity: String,
    pub workspace_identity: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LifecycleBuildReport {
    pub ok: bool,
    pub check: bool,
    pub graph_id: String,
    pub generation_id: String,
    pub release_id: String,
    pub order: Vec<String>,
    pub drift: Vec<BuildDrift>,
    pub active_generation: Option<String>,
}

pub fn run_lifecycle_build(
    options: &LifecycleBuildOptions,
) -> Result<LifecycleBuildReport, LifecycleError> {
    if options.lock_timeout.is_zero() || options.lock_timeout > MAX_LOCK_TIMEOUT {
        return Err(LifecycleError::new(
            "lifecycle lock timeout must be within 1ms..=60s",
        ));
    }
    let root = options.root.canonicalize().map_err(|error| {
        LifecycleError::new(format!(
            "failed to resolve lifecycle root `{}`: {error}",
            options.root.display()
        ))
    })?;
    if !root.is_dir() {
        return Err(LifecycleError::new("lifecycle root must be a directory"));
    }
    let catalog_path = resolve_under_root(&root, &options.catalog, "contract catalog")?;
    let config_path = resolve_under_root(&root, &options.config, "lifecycle config")?;
    let config = LifecycleBuildConfig::from_path(config_path)?;
    let catalog = ContractCatalog::from_path_at_root(&catalog_path, &root)
        .map_err(|error| LifecycleError::new(error.to_string()))?;
    let graph = LifecycleGraph::from_catalog(&catalog, &config.lifecycle())?;
    validate_executor_coverage(&graph, &config)?;

    let _lock = BuildLock::acquire(&root, options.lock_timeout)?;
    let out = resolve_output(&root, &options.out)?;
    let stage_parent = if options.check {
        None
    } else {
        let parent = out
            .parent()
            .ok_or_else(|| LifecycleError::new("lifecycle output must have a parent directory"))?;
        fs::create_dir_all(parent).map_err(io_error("create lifecycle output parent"))?;
        Some(parent)
    };
    let stage = match stage_parent {
        Some(parent) => tempfile::Builder::new()
            .prefix(".distributed-stage-")
            .tempdir_in(parent),
        None => tempfile::Builder::new()
            .prefix("distributed-check-")
            .tempdir(),
    }
    .map_err(io_error("create isolated lifecycle stage"))?;

    let order = graph.topological_order()?;
    let mut receipts = BTreeMap::new();
    for node_id in &order {
        let node = &graph.nodes[node_id];
        let executor = &config.executors[&node.executor];
        let input_identities = collect_node_inputs(&root, stage.path(), &catalog, &graph, node_id)?;
        let dependency_receipts = node
            .dependencies
            .iter()
            .map(|dependency| {
                let receipt: &ArtifactNodeReceipt = &receipts[dependency];
                (dependency.clone(), receipt.receipt_id.clone())
            })
            .collect();
        execute_node(&root, stage.path(), node_id, &node.outputs, executor)?;
        let output_identities = collect_node_outputs(stage.path(), node_id, &graph)?;
        let receipt = ArtifactNodeReceipt::new(
            node,
            executor.identity.clone(),
            input_identities,
            dependency_receipts,
            output_identities,
            true,
        )?;
        receipts.insert(node_id.clone(), receipt);
    }
    reject_unowned_stage_files(stage.path(), &graph)?;
    let generation = GenerationManifest::new(&graph, receipts.into_values())?;
    let release = ReleaseManifest::new(&graph, &generation)?;

    let drift = if options.check {
        compare_workspace_outputs(&root, &generation)?
    } else {
        Vec::new()
    };
    let active_generation = if options.check {
        read_active_generation(&out)?
    } else {
        write_manifest(
            stage.path(),
            "generation.json",
            &generation.canonical_bytes(&graph)?,
        )?;
        write_manifest(
            stage.path(),
            "release.json",
            &release.canonical_bytes(&graph, &generation)?,
        )?;
        activate_generation(stage, &out, &generation.generation_id, &release.release_id)?;
        Some(generation.generation_id.clone())
    };

    Ok(LifecycleBuildReport {
        ok: !options.check || drift.is_empty(),
        check: options.check,
        graph_id: graph.graph_id,
        generation_id: generation.generation_id,
        release_id: release.release_id,
        order,
        drift,
        active_generation,
    })
}

fn validate_executor_coverage(
    graph: &LifecycleGraph,
    config: &LifecycleBuildConfig,
) -> Result<(), LifecycleError> {
    let required = graph
        .nodes
        .values()
        .map(|node| node.executor.clone())
        .collect::<BTreeSet<_>>();
    let missing = required
        .difference(&config.executors.keys().cloned().collect())
        .cloned()
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(LifecycleError::new(format!(
            "lifecycle config is missing executors: {}",
            missing.join(", ")
        )));
    }
    Ok(())
}

fn execute_node(
    root: &Path,
    stage: &Path,
    node_id: &str,
    declared_outputs: &BTreeSet<String>,
    executor: &LifecycleExecutor,
) -> Result<(), LifecycleError> {
    let root_value = root.to_string_lossy();
    let stage_value = stage.to_string_lossy();
    let args = executor
        .args
        .iter()
        .map(|arg| {
            arg.replace("{root}", &root_value)
                .replace("{stage}", &stage_value)
                .replace("{node}", node_id)
        })
        .collect::<Vec<_>>();
    let stdout = if let Some(stdout) = &executor.stdout {
        if !declared_outputs.contains(stdout) {
            return Err(LifecycleError::new(format!(
                "lifecycle node `{node_id}` executor stdout `{stdout}` is not a declared node output"
            )));
        }
        let path = stage.join(stdout);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(io_error("create executor stdout parent"))?;
        }
        Stdio::from(File::create(path).map_err(io_error("create executor stdout output"))?)
    } else {
        Stdio::null()
    };
    let status = Command::new(&executor.program)
        .args(args)
        .current_dir(stage)
        .env("DISTRIBUTED_LIFECYCLE_ROOT", root)
        .env("DISTRIBUTED_LIFECYCLE_STAGE", stage)
        .env("DISTRIBUTED_LIFECYCLE_NODE", node_id)
        .stdout(stdout)
        .stderr(Stdio::null())
        .status()
        .map_err(|error| {
            LifecycleError::new(format!(
                "lifecycle node `{node_id}` failed to start executor `{}`: {error}",
                executor.program
            ))
        })?;
    if !status.success() {
        return Err(LifecycleError::new(format!(
            "lifecycle node `{node_id}` executor failed with {status}"
        )));
    }
    Ok(())
}

fn collect_node_inputs(
    root: &Path,
    stage: &Path,
    catalog: &ContractCatalog,
    graph: &LifecycleGraph,
    node_id: &str,
) -> Result<BTreeMap<String, String>, LifecycleError> {
    let node = &graph.nodes[node_id];
    let entry = &catalog.entries[node_id];
    let dependency_outputs = node
        .dependencies
        .iter()
        .flat_map(|dependency| graph.nodes[dependency].outputs.iter())
        .collect::<Vec<_>>();
    let mut identities = BTreeMap::new();
    for source in &node.inputs {
        let base = if dependency_outputs
            .iter()
            .any(|output| source_uses_output(source, output))
        {
            stage
        } else {
            root
        };
        for (path, identity) in hash_source(base, source, entry.provenance.glob_limit)? {
            if identities.insert(path.clone(), identity).is_some() {
                return Err(LifecycleError::new(format!(
                    "lifecycle node `{node_id}` resolves duplicate input `{path}`"
                )));
            }
        }
    }
    if identities.is_empty() {
        return Err(LifecycleError::new(format!(
            "lifecycle node `{node_id}` resolved no inputs"
        )));
    }
    Ok(identities)
}

fn collect_node_outputs(
    stage: &Path,
    node_id: &str,
    graph: &LifecycleGraph,
) -> Result<BTreeMap<String, String>, LifecycleError> {
    graph.nodes[node_id]
        .outputs
        .iter()
        .map(|output| {
            let identity = hash_path(&stage.join(output), stage)?;
            Ok((output.clone(), identity))
        })
        .collect()
}

fn hash_source(
    base: &Path,
    source: &str,
    glob_limit: Option<usize>,
) -> Result<BTreeMap<String, String>, LifecycleError> {
    validate_portable_path(source, "lifecycle source")?;
    if !source.contains(['*', '?', '[']) {
        let path = base.join(source);
        return Ok(BTreeMap::from([(
            source.to_string(),
            hash_path(&path, base)?,
        )]));
    }
    let limit = glob_limit.ok_or_else(|| {
        LifecycleError::new(format!(
            "lifecycle source glob `{source}` has no match limit"
        ))
    })?;
    let pattern = base.join(source).to_string_lossy().into_owned();
    let matches = glob_with(
        &pattern,
        MatchOptions {
            case_sensitive: true,
            require_literal_separator: true,
            require_literal_leading_dot: true,
        },
    )
    .map_err(|error| LifecycleError::new(format!("invalid lifecycle glob: {error}")))?;
    let mut identities = BTreeMap::new();
    for result in matches {
        let path = result.map_err(|error| LifecycleError::new(error.to_string()))?;
        if identities.len() >= limit || identities.len() >= MAX_HASHED_FILES {
            return Err(LifecycleError::new(format!(
                "lifecycle source glob `{source}` exceeds its match limit"
            )));
        }
        let relative = portable_relative(base, &path)?;
        identities.insert(relative, hash_path(&path, base)?);
    }
    if identities.is_empty() {
        return Err(LifecycleError::new(format!(
            "lifecycle source glob `{source}` matched no files"
        )));
    }
    Ok(identities)
}

pub(super) fn lifecycle_input_snapshot(
    root: &Path,
    catalog: &ContractCatalog,
    graph: &LifecycleGraph,
) -> Result<BTreeMap<String, String>, LifecycleError> {
    let generated = graph
        .nodes
        .values()
        .flat_map(|node| node.outputs.iter())
        .collect::<Vec<_>>();
    let mut snapshot = BTreeMap::new();
    for (node_id, node) in &graph.nodes {
        let limit = catalog.entries[node_id].provenance.glob_limit;
        for source in &node.inputs {
            if generated
                .iter()
                .any(|output| source_uses_output(source, output))
            {
                continue;
            }
            for (path, identity) in hash_source(root, source, limit)? {
                snapshot.insert(path, identity);
            }
        }
    }
    Ok(snapshot)
}

fn hash_path(path: &Path, base: &Path) -> Result<String, LifecycleError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        LifecycleError::new(format!(
            "required lifecycle path `{}` is unavailable: {error}",
            path.display()
        ))
    })?;
    if metadata.file_type().is_symlink() {
        return Err(LifecycleError::new(format!(
            "lifecycle path `{}` must not be a symlink",
            path.display()
        )));
    }
    if metadata.is_file() {
        if metadata.len() > MAX_HASHED_FILE_BYTES {
            return Err(LifecycleError::new(format!(
                "lifecycle file `{}` exceeds the 64 MiB hashing bound",
                path.display()
            )));
        }
        let mut file = File::open(path).map_err(io_error("open lifecycle file"))?;
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        file.read_to_end(&mut bytes)
            .map_err(io_error("read lifecycle file"))?;
        return Ok(digest_bytes(&bytes));
    }
    if !metadata.is_dir() {
        return Err(LifecycleError::new(format!(
            "lifecycle path `{}` is not a regular file or directory",
            path.display()
        )));
    }
    let mut files = Vec::new();
    let mut total_bytes = 0;
    collect_regular_files(path, &mut files, &mut total_bytes, 0)?;
    if files.len() > MAX_HASHED_FILES {
        return Err(LifecycleError::new(format!(
            "lifecycle directory `{}` exceeds the file bound",
            path.display()
        )));
    }
    let mut identities = BTreeMap::new();
    for file in files {
        let relative = portable_relative(base, &file)?;
        identities.insert(relative, hash_path(&file, base)?);
    }
    let bytes =
        serde_json::to_vec(&identities).map_err(|error| LifecycleError::new(error.to_string()))?;
    Ok(digest_bytes(&bytes))
}

fn collect_regular_files(
    path: &Path,
    files: &mut Vec<PathBuf>,
    total_bytes: &mut u64,
    depth: usize,
) -> Result<(), LifecycleError> {
    if depth > MAX_HASH_DEPTH {
        return Err(LifecycleError::new(format!(
            "lifecycle directory `{}` exceeds the depth bound",
            path.display()
        )));
    }
    let mut entries = fs::read_dir(path)
        .map_err(io_error("read lifecycle directory"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(io_error("read lifecycle directory entry"))?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let file_type = entry
            .file_type()
            .map_err(io_error("inspect lifecycle entry"))?;
        if file_type.is_symlink() {
            return Err(LifecycleError::new(format!(
                "lifecycle path `{}` must not contain symlinks",
                entry.path().display()
            )));
        }
        if file_type.is_dir() {
            collect_regular_files(&entry.path(), files, total_bytes, depth + 1)?;
        } else if file_type.is_file() {
            *total_bytes = total_bytes.saturating_add(
                entry
                    .metadata()
                    .map_err(io_error("inspect lifecycle file"))?
                    .len(),
            );
            if *total_bytes > MAX_HASHED_TOTAL_BYTES {
                return Err(LifecycleError::new(format!(
                    "lifecycle directory `{}` exceeds the total byte bound",
                    path.display()
                )));
            }
            files.push(entry.path());
        } else {
            return Err(LifecycleError::new(format!(
                "lifecycle path `{}` contains a special file",
                entry.path().display()
            )));
        }
        if files.len() > MAX_HASHED_FILES {
            break;
        }
    }
    Ok(())
}

fn compare_workspace_outputs(
    root: &Path,
    generation: &GenerationManifest,
) -> Result<Vec<BuildDrift>, LifecycleError> {
    let mut drift = Vec::new();
    for (node_id, receipt) in &generation.receipts {
        for (output, built_identity) in &receipt.output_identities {
            let workspace_identity = match fs::symlink_metadata(root.join(output)) {
                Ok(_) => Some(hash_path(&root.join(output), root)?),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(io_error("inspect workspace output")(error)),
            };
            if workspace_identity.as_ref() != Some(built_identity) {
                drift.push(BuildDrift {
                    node_id: node_id.clone(),
                    output: output.clone(),
                    built_identity: built_identity.clone(),
                    workspace_identity,
                });
            }
        }
    }
    Ok(drift)
}

fn reject_unowned_stage_files(stage: &Path, graph: &LifecycleGraph) -> Result<(), LifecycleError> {
    let outputs = graph
        .nodes
        .values()
        .flat_map(|node| node.outputs.iter())
        .map(Path::new)
        .collect::<Vec<_>>();
    let mut files = Vec::new();
    let mut total_bytes = 0;
    collect_regular_files(stage, &mut files, &mut total_bytes, 0)?;
    for file in files {
        let relative = portable_relative(stage, &file)?;
        let relative_path = Path::new(&relative);
        if !outputs
            .iter()
            .any(|output| relative_path == *output || relative_path.starts_with(output))
        {
            return Err(LifecycleError::new(format!(
                "executor produced unowned staged output `{relative}`"
            )));
        }
    }
    Ok(())
}

fn activate_generation(
    stage: tempfile::TempDir,
    out: &Path,
    generation_id: &str,
    release_id: &str,
) -> Result<(), LifecycleError> {
    let generations = out.join("generations");
    fs::create_dir_all(&generations).map_err(io_error("create generations directory"))?;
    let target = generations.join(generation_id);
    if target.exists() {
        let staged_identity = hash_path(stage.path(), stage.path())?;
        let active_identity = hash_path(&target, &target)?;
        if staged_identity != active_identity {
            return Err(LifecycleError::new(
                "existing generation directory disagrees with its content identity",
            ));
        }
    } else {
        fs::rename(stage.path(), &target).map_err(|error| {
            LifecycleError::new(format!(
                "failed to atomically install generation `{generation_id}`: {error}"
            ))
        })?;
    }
    let active = serde_json::to_vec(&serde_json::json!({
        "schema_version": 1,
        "generation_id": generation_id,
        "release_id": release_id,
        "path": format!("generations/{generation_id}")
    }))
    .map_err(|error| LifecycleError::new(error.to_string()))?;
    fs::create_dir_all(out).map_err(io_error("create lifecycle output"))?;
    let mut pointer = tempfile::NamedTempFile::new_in(out)
        .map_err(io_error("create active generation pointer"))?;
    pointer
        .write_all(&active)
        .map_err(io_error("write active generation pointer"))?;
    pointer
        .as_file()
        .sync_all()
        .map_err(io_error("sync active generation pointer"))?;
    pointer
        .persist(out.join("active.json"))
        .map_err(|error| io_error("activate generation pointer")(error.error))?;
    Ok(())
}

fn write_manifest(stage: &Path, name: &str, bytes: &[u8]) -> Result<(), LifecycleError> {
    fs::write(stage.join(name), bytes).map_err(io_error("write lifecycle manifest"))
}

fn read_active_generation(out: &Path) -> Result<Option<String>, LifecycleError> {
    let path = out.join("active.json");
    if path
        .metadata()
        .is_ok_and(|metadata| metadata.len() > 1024 * 1024)
    {
        return Err(LifecycleError::new(
            "active generation pointer exceeds 1 MiB",
        ));
    }
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_error("read active generation pointer")(error)),
    };
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|error| {
        LifecycleError::new(format!("invalid active generation pointer: {error}"))
    })?;
    Ok(value
        .get("generation_id")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string))
}

#[derive(Debug)]
struct BuildLock {
    file: File,
}

impl BuildLock {
    fn acquire(root: &Path, timeout: Duration) -> Result<Self, LifecycleError> {
        let identity = digest_bytes(root.to_string_lossy().as_bytes());
        let name = format!("distributed-lifecycle-{}.lock", &identity[7..23]);
        let path = std::env::temp_dir().join(name);
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)
            .map_err(io_error("open lifecycle process lock"))?;
        let started = Instant::now();
        loop {
            match file.try_lock_exclusive() {
                Ok(()) => return Ok(Self { file }),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if started.elapsed() >= timeout {
                        return Err(LifecycleError::new(format!(
                            "timed out after {}ms waiting for lifecycle process lock",
                            timeout.as_millis()
                        )));
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => return Err(io_error("acquire lifecycle process lock")(error)),
            }
        }
    }
}

impl Drop for BuildLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

fn resolve_under_root(root: &Path, path: &Path, label: &str) -> Result<PathBuf, LifecycleError> {
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    reject_symlink_components(root, &candidate, label)?;
    let resolved = candidate.canonicalize().map_err(|error| {
        LifecycleError::new(format!(
            "failed to resolve {label} `{}`: {error}",
            candidate.display()
        ))
    })?;
    if !resolved.starts_with(root) {
        return Err(LifecycleError::new(format!(
            "{label} escapes lifecycle root"
        )));
    }
    Ok(resolved)
}

fn resolve_output(root: &Path, path: &Path) -> Result<PathBuf, LifecycleError> {
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(LifecycleError::new(
            "lifecycle output must not contain parent-directory components",
        ));
    }
    let output = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    if output == root || !output.starts_with(root) {
        return Err(LifecycleError::new(
            "lifecycle output must be a child of the lifecycle root",
        ));
    }
    reject_symlink_components(root, &output, "lifecycle output")?;
    let mut ancestor = output.as_path();
    while !ancestor.exists() {
        ancestor = ancestor
            .parent()
            .ok_or_else(|| LifecycleError::new("lifecycle output has no existing ancestor"))?;
    }
    let resolved_ancestor = ancestor
        .canonicalize()
        .map_err(io_error("resolve lifecycle output ancestor"))?;
    if !resolved_ancestor.starts_with(root) {
        return Err(LifecycleError::new(
            "lifecycle output escapes lifecycle root",
        ));
    }
    Ok(output)
}

fn reject_symlink_components(root: &Path, path: &Path, label: &str) -> Result<(), LifecycleError> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| LifecycleError::new(format!("{label} escapes lifecycle root")))?;
    let mut candidate = root.to_path_buf();
    for component in relative.components() {
        candidate.push(component);
        match fs::symlink_metadata(&candidate) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(LifecycleError::new(format!(
                    "{label} `{}` crosses a symlink",
                    candidate.display()
                )));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => return Err(io_error("inspect lifecycle path component")(error)),
        }
    }
    Ok(())
}

fn portable_relative(base: &Path, path: &Path) -> Result<String, LifecycleError> {
    let relative = path.strip_prefix(base).map_err(|_| {
        LifecycleError::new(format!("path `{}` escapes lifecycle root", path.display()))
    })?;
    let value = relative.to_str().ok_or_else(|| {
        LifecycleError::new(format!("path `{}` is not valid UTF-8", relative.display()))
    })?;
    validate_portable_path(value, "lifecycle path")?;
    Ok(value.to_string())
}

fn source_uses_output(source: &str, output: &str) -> bool {
    let prefix = source
        .split(['*', '?', '['])
        .next()
        .unwrap_or(source)
        .trim_end_matches('/');
    prefix == output || prefix.starts_with(&format!("{output}/"))
}

fn io_error(label: &'static str) -> impl FnOnce(std::io::Error) -> LifecycleError {
    move |error| LifecycleError::new(format!("{label}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrent_build_lock_wait_is_bounded() {
        let root = tempfile::tempdir().expect("create lock fixture root");
        let _first = BuildLock::acquire(root.path(), Duration::from_millis(100))
            .expect("acquire first lifecycle lock");
        let error = BuildLock::acquire(root.path(), Duration::from_millis(20))
            .expect_err("second lifecycle lock should time out");
        assert!(error.message().contains("timed out"));
    }
}
