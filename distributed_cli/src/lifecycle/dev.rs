use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::contracts::ContractCatalog;

use super::build::lifecycle_input_snapshot;
use super::{
    run_lifecycle_build, validate_stable_value, LifecycleBuildConfig, LifecycleBuildOptions,
    LifecycleBuildReport, LifecycleError, LifecycleGraph,
};

const MAX_DEV_PROCESSES: usize = 64;
const MAX_DEV_INTERVAL: Duration = Duration::from_secs(30);

fn default_poll_ms() -> u64 {
    200
}

fn default_debounce_ms() -> u64 {
    100
}

fn default_ready_after_ms() -> u64 {
    100
}

fn default_shutdown_ms() -> u64 {
    5_000
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LifecycleDevProcess {
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// Restart after a successful generation when any named node invalidates.
    /// An empty set leaves native HMR/process watching entirely in charge.
    #[serde(default)]
    pub restart_on: BTreeSet<String>,
    #[serde(default = "default_ready_after_ms")]
    pub ready_after_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LifecycleDevConfig {
    #[serde(default = "default_poll_ms")]
    pub poll_ms: u64,
    #[serde(default = "default_debounce_ms")]
    pub debounce_ms: u64,
    #[serde(default = "default_shutdown_ms")]
    pub shutdown_ms: u64,
    pub processes: BTreeMap<String, LifecycleDevProcess>,
}

impl LifecycleDevConfig {
    pub fn validate(&self) -> Result<(), LifecycleError> {
        for (label, value) in [
            ("poll", self.poll_ms),
            ("debounce", self.debounce_ms),
            ("shutdown", self.shutdown_ms),
        ] {
            if value == 0 || Duration::from_millis(value) > MAX_DEV_INTERVAL {
                return Err(LifecycleError::new(format!(
                    "lifecycle dev {label} interval must be within 1ms..=30s"
                )));
            }
        }
        if self.processes.is_empty() || self.processes.len() > MAX_DEV_PROCESSES {
            return Err(LifecycleError::new(format!(
                "lifecycle dev processes must contain 1..={MAX_DEV_PROCESSES} entries"
            )));
        }
        if self
            .processes
            .values()
            .map(|process| process.ready_after_ms)
            .sum::<u64>()
            > MAX_DEV_INTERVAL.as_millis() as u64
        {
            return Err(LifecycleError::new(
                "total lifecycle dev readiness delay must not exceed 30s",
            ));
        }
        for (name, process) in &self.processes {
            validate_stable_value(name, "lifecycle dev process name")?;
            validate_stable_value(&process.program, "lifecycle dev process program")?;
            if process.args.len() > 256
                || process.args.iter().map(String::len).sum::<usize>() > 16 * 1024
                || process.args.iter().any(|arg| arg.contains('\0'))
            {
                return Err(LifecycleError::new(format!(
                    "lifecycle dev process `{name}` exceeds argument bounds"
                )));
            }
            if process.ready_after_ms == 0
                || Duration::from_millis(process.ready_after_ms) > MAX_DEV_INTERVAL
            {
                return Err(LifecycleError::new(format!(
                    "lifecycle dev process `{name}` readiness interval is out of bounds"
                )));
            }
            for node in &process.restart_on {
                validate_stable_value(node, "lifecycle dev restart node")?;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct LifecycleDevOptions {
    pub build: LifecycleBuildOptions,
    pub stop: Arc<AtomicBool>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LifecycleDevReport {
    pub initial_generation: String,
    pub final_generation: String,
    pub rebuilds: usize,
    pub restarts: BTreeMap<String, usize>,
}

pub fn run_lifecycle_dev(
    options: &LifecycleDevOptions,
) -> Result<LifecycleDevReport, LifecycleError> {
    if options.build.check {
        return Err(LifecycleError::new(
            "lifecycle dev requires an activating build configuration",
        ));
    }
    let root = options.build.root.canonicalize().map_err(|error| {
        LifecycleError::new(format!("failed to resolve lifecycle dev root: {error}"))
    })?;
    let config_path = resolve_file(&root, &options.build.config)?;
    let catalog_path = resolve_file(&root, &options.build.catalog)?;
    let config = LifecycleBuildConfig::from_path(config_path)?;
    let dev = config.dev.clone().ok_or_else(|| {
        LifecycleError::new("lifecycle config has no `dev` process configuration")
    })?;
    dev.validate()?;
    let catalog = ContractCatalog::from_path_at_root(&catalog_path, &root)
        .map_err(|error| LifecycleError::new(error.to_string()))?;
    let graph = LifecycleGraph::from_catalog(&catalog, &config.lifecycle())?;
    validate_restart_nodes(&dev, &graph)?;

    // Initial coherent generation is an absolute serving barrier.
    let mut initial_options = options.build.clone();
    initial_options.nodes = None;
    initial_options.activation_inputs = None;
    let initial = run_lifecycle_build(&initial_options)?;
    let mut children = ChildSet::start(&root, &dev, &initial)?;
    let mut snapshot = lifecycle_input_snapshot(&root, &catalog, &graph)?;
    let mut final_generation = initial.generation_id.clone();
    let mut rebuilds = 0;
    let mut restarts = dev
        .processes
        .keys()
        .map(|name| (name.clone(), 0))
        .collect::<BTreeMap<_, _>>();

    let result = (|| {
        while !options.stop.load(Ordering::SeqCst) {
            children.ensure_running()?;
            std::thread::sleep(Duration::from_millis(dev.poll_ms));
            let next = lifecycle_input_snapshot(&root, &catalog, &graph)?;
            let mut changed = changed_paths(&snapshot, &next);
            if changed.is_empty() {
                continue;
            }

            // Quiet-period debounce; each new content identity extends the window.
            let mut quiet_since = Instant::now();
            let mut latest = next;
            while quiet_since.elapsed() < Duration::from_millis(dev.debounce_ms) {
                if options.stop.load(Ordering::SeqCst) {
                    return Ok(());
                }
                std::thread::sleep(Duration::from_millis(dev.poll_ms.min(dev.debounce_ms)));
                let observed = lifecycle_input_snapshot(&root, &catalog, &graph)?;
                let delta = changed_paths(&latest, &observed);
                if !delta.is_empty() {
                    changed.extend(delta);
                    latest = observed;
                    quiet_since = Instant::now();
                }
            }

            let invalidated = graph.invalidated_by_paths(&changed)?;
            let mut rebuild_options = options.build.clone();
            rebuild_options.nodes = Some(invalidated.clone());
            rebuild_options.activation_inputs = Some(latest.clone());
            let generation = match run_lifecycle_build(&rebuild_options) {
                Ok(generation) => generation,
                Err(error) if error.message().contains("was superseded") => continue,
                Err(error) => return Err(error),
            };
            rebuilds += 1;
            final_generation = generation.generation_id.clone();
            children.restart_invalidated(&root, &dev, &generation, &invalidated, &mut restarts)?;
            snapshot = latest;
        }
        Ok(())
    })();

    let shutdown = children.shutdown(Duration::from_millis(dev.shutdown_ms));
    result?;
    shutdown?;
    Ok(LifecycleDevReport {
        initial_generation: initial.generation_id,
        final_generation,
        rebuilds,
        restarts,
    })
}

fn validate_restart_nodes(
    dev: &LifecycleDevConfig,
    graph: &LifecycleGraph,
) -> Result<(), LifecycleError> {
    for (name, process) in &dev.processes {
        let missing = process
            .restart_on
            .iter()
            .filter(|node| !graph.nodes.contains_key(*node))
            .cloned()
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            return Err(LifecycleError::new(format!(
                "lifecycle dev process `{name}` references unknown restart nodes: {}",
                missing.join(", ")
            )));
        }
    }
    Ok(())
}

fn changed_paths(
    before: &BTreeMap<String, String>,
    after: &BTreeMap<String, String>,
) -> BTreeSet<String> {
    before
        .keys()
        .chain(after.keys())
        .filter(|path| before.get(*path) != after.get(*path))
        .cloned()
        .collect()
}

struct ChildSet {
    children: BTreeMap<String, Child>,
}

impl ChildSet {
    fn start(
        root: &Path,
        config: &LifecycleDevConfig,
        generation: &LifecycleBuildReport,
    ) -> Result<Self, LifecycleError> {
        let mut set = Self {
            children: BTreeMap::new(),
        };
        for (name, process) in &config.processes {
            match spawn_process(root, name, process, generation) {
                Ok(child) => {
                    set.children.insert(name.clone(), child);
                }
                Err(error) => {
                    let _ = set.shutdown(Duration::from_secs(1));
                    return Err(error);
                }
            }
        }
        for (name, process) in &config.processes {
            std::thread::sleep(Duration::from_millis(process.ready_after_ms));
            if let Some(status) =
                set.children
                    .get_mut(name)
                    .unwrap()
                    .try_wait()
                    .map_err(|error| {
                        LifecycleError::new(format!(
                            "failed to inspect dev process `{name}`: {error}"
                        ))
                    })?
            {
                let _ = set.shutdown(Duration::from_secs(1));
                return Err(LifecycleError::new(format!(
                    "lifecycle dev process `{name}` exited before readiness with {status}"
                )));
            }
        }
        Ok(set)
    }

    fn ensure_running(&mut self) -> Result<(), LifecycleError> {
        for (name, child) in &mut self.children {
            if let Some(status) = child.try_wait().map_err(|error| {
                LifecycleError::new(format!("failed to inspect dev process `{name}`: {error}"))
            })? {
                return Err(LifecycleError::new(format!(
                    "lifecycle dev process `{name}` exited with {status}"
                )));
            }
        }
        Ok(())
    }

    fn restart_invalidated(
        &mut self,
        root: &Path,
        config: &LifecycleDevConfig,
        generation: &LifecycleBuildReport,
        invalidated: &BTreeSet<String>,
        restarts: &mut BTreeMap<String, usize>,
    ) -> Result<(), LifecycleError> {
        for (name, process) in &config.processes {
            if process.restart_on.is_disjoint(invalidated) {
                continue;
            }
            stop_child(name, self.children.get_mut(name).unwrap())?;
            let child = spawn_process(root, name, process, generation)?;
            self.children.insert(name.clone(), child);
            *restarts.get_mut(name).unwrap() += 1;
        }
        Ok(())
    }

    fn shutdown(&mut self, timeout: Duration) -> Result<(), LifecycleError> {
        for child in self.children.values_mut() {
            if child.try_wait().map_err(io_error)?.is_none() {
                child.kill().map_err(io_error)?;
            }
        }
        let started = Instant::now();
        for (name, child) in &mut self.children {
            while child.try_wait().map_err(io_error)?.is_none() {
                if started.elapsed() >= timeout {
                    return Err(LifecycleError::new(format!(
                        "timed out shutting down lifecycle dev process `{name}`"
                    )));
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        Ok(())
    }
}

fn spawn_process(
    root: &Path,
    name: &str,
    process: &LifecycleDevProcess,
    generation: &LifecycleBuildReport,
) -> Result<Child, LifecycleError> {
    let root_value = root.to_string_lossy();
    let args = process
        .args
        .iter()
        .map(|arg| {
            arg.replace("{root}", &root_value)
                .replace("{process}", name)
                .replace("{generation}", &generation.generation_id)
        })
        .collect::<Vec<_>>();
    Command::new(&process.program)
        .args(args)
        .current_dir(root)
        .env("DISTRIBUTED_GENERATION_ID", &generation.generation_id)
        .env("DISTRIBUTED_RELEASE_ID", &generation.release_id)
        .stdin(Stdio::null())
        .spawn()
        .map_err(|error| {
            LifecycleError::new(format!(
                "failed to start lifecycle dev process `{name}` with `{}`: {error}",
                process.program
            ))
        })
}

fn stop_child(name: &str, child: &mut Child) -> Result<(), LifecycleError> {
    if child.try_wait().map_err(io_error)?.is_none() {
        child.kill().map_err(|error| {
            LifecycleError::new(format!(
                "failed to stop lifecycle dev process `{name}`: {error}"
            ))
        })?;
        child.wait().map_err(io_error)?;
    }
    Ok(())
}

fn resolve_file(root: &Path, path: &PathBuf) -> Result<PathBuf, LifecycleError> {
    let path = if path.is_absolute() {
        path.clone()
    } else {
        root.join(path)
    };
    let resolved = path.canonicalize().map_err(|error| {
        LifecycleError::new(format!("failed to resolve `{}`: {error}", path.display()))
    })?;
    if !resolved.starts_with(root)
        || !fs::metadata(&resolved).is_ok_and(|metadata| metadata.is_file())
    {
        return Err(LifecycleError::new(
            "lifecycle dev input must be a file under the root",
        ));
    }
    Ok(resolved)
}

fn io_error(error: std::io::Error) -> LifecycleError {
    LifecycleError::new(error.to_string())
}
