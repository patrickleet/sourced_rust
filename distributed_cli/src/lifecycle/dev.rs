use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::contracts::ContractCatalog;

use super::build::lifecycle_input_snapshot;
use super::graph::LifecycleErrorReason;
use super::{
    run_lifecycle_build, validate_portable_path, validate_stable_value, LifecycleBuildConfig,
    LifecycleBuildOptions, LifecycleBuildReport, LifecycleError, LifecycleGraph,
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
    /// Working directory relative to the lifecycle root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Process-specific environment. Values may use lifecycle placeholders.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// Operator-facing URL printed after readiness succeeds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Restart after a successful generation when any named node invalidates.
    /// An empty set leaves native HMR/process watching entirely in charge.
    #[serde(default)]
    pub restart_on: BTreeSet<String>,
    #[serde(default = "default_ready_after_ms")]
    pub ready_after_ms: u64,
    /// Optional command probe; exit status 0 is readiness evidence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ready: Option<LifecycleDevProbe>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LifecycleDevProbe {
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default = "default_poll_ms")]
    pub interval_ms: u64,
    #[serde(default = "default_shutdown_ms")]
    pub timeout_ms: u64,
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
            .map(readiness_budget_ms)
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
            if let Some(cwd) = &process.cwd {
                validate_portable_path(cwd, "lifecycle dev process cwd")?;
            }
            validate_process_env(name, &process.env)?;
            if let Some(url) = &process.url {
                validate_stable_value(url, "lifecycle dev process URL")?;
            }
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
            if let Some(probe) = &process.ready {
                validate_stable_value(&probe.program, "lifecycle readiness probe program")?;
                if probe.args.len() > 256
                    || probe.args.iter().map(String::len).sum::<usize>() > 16 * 1024
                    || probe.args.iter().any(|arg| arg.contains('\0'))
                    || probe.interval_ms == 0
                    || probe.timeout_ms == 0
                    || probe.interval_ms > probe.timeout_ms
                    || Duration::from_millis(probe.timeout_ms) > MAX_DEV_INTERVAL
                {
                    return Err(LifecycleError::new(format!(
                        "lifecycle dev process `{name}` readiness probe is out of bounds"
                    )));
                }
            }
            for node in &process.restart_on {
                validate_stable_value(node, "lifecycle dev restart node")?;
            }
        }
        Ok(())
    }
}

fn readiness_budget_ms(process: &LifecycleDevProcess) -> u64 {
    process
        .ready
        .as_ref()
        .map_or(process.ready_after_ms, |probe| probe.timeout_ms)
}

#[derive(Clone, Debug)]
pub struct LifecycleDevOptions {
    pub build: LifecycleBuildOptions,
    pub stop: Arc<AtomicBool>,
    /// Emit operator-facing readiness and rebuild progress to stderr.
    pub progress: bool,
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
    initial_options.cancel = Some(Arc::clone(&options.stop));
    let initial = run_lifecycle_build(&initial_options)?;
    let mut children = ChildSet::start(&root, &dev, &initial)?;
    if options.progress {
        for (name, process) in &dev.processes {
            if let Some(url) = &process.url {
                eprintln!("lifecycle dev: process {name} ready {url}");
            }
        }
        eprintln!(
            "lifecycle dev: ready generation={} processes={} (Ctrl-C to stop)",
            initial.generation_id,
            dev.processes.keys().cloned().collect::<Vec<_>>().join(",")
        );
    }
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
            let submitted_inputs = latest.clone();
            rebuild_options.activation_inputs = Some(submitted_inputs.clone());
            let cancel = Arc::new(AtomicBool::new(false));
            rebuild_options.cancel = Some(Arc::clone(&cancel));
            let build = std::thread::spawn(move || run_lifecycle_build(&rebuild_options));
            let mut superseded = false;
            while !build.is_finished() {
                if options.stop.load(Ordering::SeqCst) {
                    cancel.store(true, Ordering::SeqCst);
                }
                if let Err(error) = children.ensure_running() {
                    cancel.store(true, Ordering::SeqCst);
                    let _ = build.join();
                    return Err(error);
                }
                std::thread::sleep(Duration::from_millis(dev.poll_ms));
                let observed = match lifecycle_input_snapshot(&root, &catalog, &graph) {
                    Ok(observed) => observed,
                    Err(error) => {
                        cancel.store(true, Ordering::SeqCst);
                        let _ = build.join();
                        return Err(error);
                    }
                };
                if !changed_paths(&latest, &observed).is_empty() {
                    latest = observed;
                    superseded = true;
                    cancel.store(true, Ordering::SeqCst);
                }
            }
            let build_result = build
                .join()
                .map_err(|_| LifecycleError::new("lifecycle build worker panicked"))?;
            let generation = match build_result {
                Ok(generation) => generation,
                Err(error)
                    if options.stop.load(Ordering::SeqCst)
                        && error.reason() == LifecycleErrorReason::Canceled =>
                {
                    return Ok(())
                }
                Err(error)
                    if superseded
                        && matches!(
                            error.reason(),
                            LifecycleErrorReason::Canceled | LifecycleErrorReason::Superseded
                        ) =>
                {
                    continue
                }
                Err(error) if error.reason() == LifecycleErrorReason::Superseded => continue,
                Err(error) => return Err(error),
            };
            rebuilds += 1;
            final_generation = generation.generation_id.clone();
            if options.stop.load(Ordering::SeqCst) {
                return Ok(());
            }
            let restarting = dev
                .processes
                .iter()
                .filter(|(_, process)| !process.restart_on.is_disjoint(&invalidated))
                .map(|(name, _)| name.clone())
                .collect::<Vec<_>>();
            children.restart_invalidated(&root, &dev, &generation, &invalidated, &mut restarts)?;
            if options.progress {
                eprintln!(
                    "lifecycle dev: activated generation={} invalidated={} restarted={}",
                    generation.generation_id,
                    invalidated.iter().cloned().collect::<Vec<_>>().join(","),
                    if restarting.is_empty() {
                        "none".to_string()
                    } else {
                        restarting.join(",")
                    }
                );
            }
            snapshot = submitted_inputs;
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
            if let Err(error) = wait_ready(
                root,
                name,
                process,
                generation,
                set.children.get_mut(name).unwrap(),
            ) {
                let _ = set.shutdown(Duration::from_secs(1));
                return Err(error);
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
            stop_child(
                name,
                self.children.get_mut(name).unwrap(),
                Duration::from_millis(config.shutdown_ms),
            )?;
            let mut child = spawn_process(root, name, process, generation)?;
            if let Err(error) = wait_ready(root, name, process, generation, &mut child) {
                let _ = stop_child(name, &mut child, Duration::from_millis(config.shutdown_ms));
                return Err(error);
            }
            self.children.insert(name.clone(), child);
            *restarts.get_mut(name).unwrap() += 1;
        }
        Ok(())
    }

    fn shutdown(&mut self, timeout: Duration) -> Result<(), LifecycleError> {
        for (name, child) in &mut self.children {
            terminate_process_group(name, child)?;
        }
        let started = Instant::now();
        loop {
            let mut running = Vec::new();
            for (name, child) in &mut self.children {
                if child.try_wait().map_err(io_error)?.is_none() {
                    running.push(name.clone());
                }
            }
            if running.is_empty() {
                return Ok(());
            }
            if started.elapsed() >= timeout {
                for name in running {
                    let child = self.children.get_mut(&name).unwrap();
                    kill_process_group(&name, child)?;
                    child.wait().map_err(io_error)?;
                }
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

fn wait_ready(
    root: &Path,
    name: &str,
    process: &LifecycleDevProcess,
    generation: &LifecycleBuildReport,
    child: &mut Child,
) -> Result<(), LifecycleError> {
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().map_err(|error| {
            LifecycleError::new(format!("failed to inspect dev process `{name}`: {error}"))
        })? {
            return Err(LifecycleError::new(format!(
                "lifecycle dev process `{name}` exited before readiness with {status}"
            )));
        }
        let Some(probe) = &process.ready else {
            std::thread::sleep(Duration::from_millis(process.ready_after_ms));
            return Ok(());
        };
        let cwd = resolve_working_dir(root, process.cwd.as_deref())?;
        let environment = expand_environment(root, name, process, generation);
        let args = probe
            .args
            .iter()
            .map(|arg| expand_process_value(arg, root, name, generation))
            .collect::<Vec<_>>();
        let status = Command::new(expand_process_value(&probe.program, root, name, generation))
            .args(args)
            .current_dir(cwd)
            .envs(environment)
            .env("DISTRIBUTED_GENERATION_ID", &generation.generation_id)
            .env("DISTRIBUTED_RELEASE_ID", &generation.release_id)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|error| {
                LifecycleError::new(format!(
                    "failed to run readiness probe for lifecycle dev process `{name}`: {error}"
                ))
            })?;
        if status.success() {
            return Ok(());
        }
        if started.elapsed() >= Duration::from_millis(probe.timeout_ms) {
            return Err(LifecycleError::new(format!(
                "readiness probe for lifecycle dev process `{name}` timed out after {}ms",
                probe.timeout_ms
            )));
        }
        std::thread::sleep(Duration::from_millis(probe.interval_ms));
    }
}

fn spawn_process(
    root: &Path,
    name: &str,
    process: &LifecycleDevProcess,
    generation: &LifecycleBuildReport,
) -> Result<Child, LifecycleError> {
    let cwd = resolve_working_dir(root, process.cwd.as_deref())?;
    let args = process
        .args
        .iter()
        .map(|arg| expand_process_value(arg, root, name, generation))
        .collect::<Vec<_>>();
    let program = expand_process_value(&process.program, root, name, generation);
    let mut command = Command::new(&program);
    command
        .args(args)
        .current_dir(cwd)
        .envs(expand_environment(root, name, process, generation))
        .env("DISTRIBUTED_GENERATION_ID", &generation.generation_id)
        .env("DISTRIBUTED_RELEASE_ID", &generation.release_id)
        .stdin(Stdio::null());
    #[cfg(unix)]
    command.process_group(0);
    command.spawn().map_err(|error| {
        LifecycleError::new(format!(
            "failed to start lifecycle dev process `{name}` with `{program}`: {error}"
        ))
    })
}

fn stop_child(name: &str, child: &mut Child, timeout: Duration) -> Result<(), LifecycleError> {
    terminate_process_group(name, child)?;
    let started = Instant::now();
    while child.try_wait().map_err(io_error)?.is_none() {
        if started.elapsed() >= timeout {
            kill_process_group(name, child)?;
            child.wait().map_err(io_error)?;
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}

fn expand_process_value(
    value: &str,
    root: &Path,
    name: &str,
    generation: &LifecycleBuildReport,
) -> String {
    value
        .replace("{root}", &root.to_string_lossy())
        .replace("{process}", name)
        .replace("{generation}", &generation.generation_id)
}

fn expand_environment(
    root: &Path,
    name: &str,
    process: &LifecycleDevProcess,
    generation: &LifecycleBuildReport,
) -> BTreeMap<String, String> {
    process
        .env
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                expand_process_value(value, root, name, generation),
            )
        })
        .collect()
}

fn resolve_working_dir(root: &Path, cwd: Option<&str>) -> Result<PathBuf, LifecycleError> {
    let path = cwd.map_or_else(|| root.to_path_buf(), |cwd| root.join(cwd));
    let resolved = path.canonicalize().map_err(|error| {
        LifecycleError::new(format!(
            "failed to resolve lifecycle dev working directory `{}`: {error}",
            path.display()
        ))
    })?;
    if !resolved.starts_with(root)
        || !fs::metadata(&resolved).is_ok_and(|metadata| metadata.is_dir())
    {
        return Err(LifecycleError::new(
            "lifecycle dev working directory must be a directory under the root",
        ));
    }
    Ok(resolved)
}

fn validate_process_env(
    process_name: &str,
    environment: &BTreeMap<String, String>,
) -> Result<(), LifecycleError> {
    let valid_key = |key: &str| {
        let mut bytes = key.bytes();
        bytes
            .next()
            .is_some_and(|byte| byte == b'_' || byte.is_ascii_alphabetic())
            && bytes.all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
    };
    if environment.len() > 64
        || environment
            .iter()
            .map(|(key, value)| key.len() + value.len())
            .sum::<usize>()
            > 16 * 1024
        || environment
            .iter()
            .any(|(key, value)| !valid_key(key) || value.contains('\0'))
    {
        return Err(LifecycleError::new(format!(
            "lifecycle dev process `{process_name}` has invalid or oversized environment"
        )));
    }
    Ok(())
}

#[cfg(unix)]
fn signal_process_group(child: &Child, signal: libc::c_int) -> std::io::Result<()> {
    let result = unsafe { libc::kill(-(child.id() as libc::pid_t), signal) };
    if result == 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(())
    } else {
        Err(error)
    }
}

fn terminate_process_group(name: &str, child: &mut Child) -> Result<(), LifecycleError> {
    #[cfg(unix)]
    {
        signal_process_group(child, libc::SIGTERM).map_err(|error| {
            LifecycleError::new(format!(
                "failed to stop lifecycle dev process group `{name}`: {error}"
            ))
        })
    }
    #[cfg(not(unix))]
    {
        if child.try_wait().map_err(io_error)?.is_none() {
            child.kill().map_err(|error| {
                LifecycleError::new(format!(
                    "failed to stop lifecycle dev process `{name}`: {error}"
                ))
            })?;
        }
        Ok(())
    }
}

fn kill_process_group(name: &str, child: &mut Child) -> Result<(), LifecycleError> {
    #[cfg(unix)]
    {
        signal_process_group(child, libc::SIGKILL).map_err(|error| {
            LifecycleError::new(format!(
                "failed to kill lifecycle dev process group `{name}`: {error}"
            ))
        })
    }
    #[cfg(not(unix))]
    {
        if child.try_wait().map_err(io_error)?.is_none() {
            child.kill().map_err(|error| {
                LifecycleError::new(format!(
                    "failed to kill lifecycle dev process `{name}`: {error}"
                ))
            })?;
        }
        Ok(())
    }
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
