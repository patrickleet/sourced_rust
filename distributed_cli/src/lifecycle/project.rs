use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::contracts::{
    ArtifactIdentity, ArtifactProvenance, ContractArtifactKind, ContractCatalog, ContractEntry,
    ContractScope, CONTRACT_CATALOG_SCHEMA_VERSION,
};

use super::{
    digest_bytes, DistributedSourceIdentity, LifecycleBuildConfig, LifecycleDevConfig,
    LifecycleDevProbe, LifecycleDevProcess, LifecycleError, LifecycleExecutor,
    LifecycleProjectPlan, LIFECYCLE_BUILD_CONFIG_SCHEMA_VERSION,
};

const APPLICATION_NODE: &str = "application";
const APPLICATION_OUTPUT: &str = "artifacts/application-manifest.json";

/// Cargo- and convention-derived inputs for one Distributed application.
#[derive(Clone, Debug)]
pub struct DiscoveredLifecycleProject {
    /// Stable project name derived from the Cargo workspace directory.
    pub name: String,
    /// Package exporting the typed `distributed::ApplicationManifest`.
    pub application_package: String,
    /// Fully-qualified zero-argument Rust export used for typed introspection.
    pub application_entrypoint: String,
    /// Resolved source directory of the `distributed` Rust dependency.
    pub distributed_root: PathBuf,
    /// Cargo package containing the local runtime binary.
    pub runtime_package: String,
    /// Binary started by `distributed dev` and compiled by `distributed build`.
    pub runtime_binary: String,
    /// Conventional SvelteKit root, when `ui/package.json` exists.
    pub ui: Option<PathBuf>,
    /// Internal lifecycle graph synthesized from the discovered project.
    pub plan: LifecycleProjectPlan,
}

#[derive(Debug, Deserialize)]
struct CargoMetadata {
    workspace_root: PathBuf,
    workspace_members: Vec<String>,
    packages: Vec<CargoPackage>,
    #[serde(default)]
    metadata: Value,
}

#[derive(Debug, Deserialize)]
struct CargoPackage {
    id: String,
    name: String,
    version: String,
    manifest_path: PathBuf,
    #[serde(default)]
    metadata: Value,
    targets: Vec<CargoTarget>,
}

#[derive(Debug, Deserialize)]
struct CargoTarget {
    name: String,
    kind: Vec<String>,
}

#[derive(Debug)]
struct ApplicationTarget {
    package: String,
    crate_name: String,
    entrypoint: String,
}

#[derive(Debug)]
struct RuntimeTarget {
    package: String,
    binary: String,
}

/// Discover a Distributed application without a user-authored lifecycle graph.
pub fn discover_lifecycle_project(
    requested: impl AsRef<Path>,
    executable: impl AsRef<Path>,
    command_prefix: &[&str],
    out: Option<&Path>,
) -> Result<DiscoveredLifecycleProject, LifecycleError> {
    let requested = requested.as_ref();
    let requested = requested.canonicalize().map_err(|error| {
        LifecycleError::new(format!(
            "failed to resolve Distributed project `{}`: {error}",
            requested.display()
        ))
    })?;
    if !requested.is_dir() {
        return Err(LifecycleError::new(format!(
            "Distributed project `{}` is not a directory",
            requested.display()
        )));
    }
    let manifest = requested.join("Cargo.toml");
    if !manifest.is_file() {
        return Err(LifecycleError::new(format!(
            "Distributed project `{}` has no Cargo.toml",
            requested.display()
        )));
    }

    let metadata = cargo_metadata(&manifest)?;
    let root = metadata.workspace_root.canonicalize().map_err(|error| {
        LifecycleError::new(format!("failed to resolve Cargo workspace root: {error}"))
    })?;
    if root != requested {
        return Err(LifecycleError::new(format!(
            "project path `{}` resolves to Cargo workspace `{}`; run the command from the workspace root",
            requested.display(),
            root.display()
        )));
    }
    let workspace_ids = metadata
        .workspace_members
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let workspace_packages = metadata
        .packages
        .iter()
        .filter(|package| workspace_ids.contains(package.id.as_str()))
        .collect::<Vec<_>>();
    let application = discover_application(&metadata.metadata, &workspace_packages)?;
    let runtime = discover_runtime(&metadata.metadata, &workspace_packages)?;
    let distributed = metadata
        .packages
        .iter()
        .find(|package| package.name == "distributed")
        .ok_or_else(|| {
            LifecycleError::new("Cargo workspace does not depend on the `distributed` Rust package")
        })?;
    let distributed_root = distributed
        .manifest_path
        .parent()
        .ok_or_else(|| LifecycleError::new("Distributed Cargo.toml has no parent directory"))?
        .to_path_buf();
    let executable = executable.as_ref().canonicalize().map_err(|error| {
        LifecycleError::new(format!(
            "failed to resolve the distributed executable: {error}"
        ))
    })?;
    let project_name = root
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or(&application.package)
        .to_string();
    let sources = workspace_sources(&root, &workspace_packages)?;
    let source_identity = source_identity(distributed, &executable)?;
    let executor_identity = digest_bytes(
        format!(
            "distributed-describe-v1\0{}\0{}\0{}",
            source_identity, application.package, application.entrypoint
        )
        .as_bytes(),
    );
    let entrypoint = if application.entrypoint.contains("::") {
        application.entrypoint.clone()
    } else {
        format!("{}::{}", application.crate_name, application.entrypoint)
    };

    let catalog = ContractCatalog {
        schema_version: CONTRACT_CATALOG_SCHEMA_VERSION,
        entries: BTreeMap::from([(
            APPLICATION_NODE.to_string(),
            ContractEntry {
                id: APPLICATION_NODE.to_string(),
                kind: ContractArtifactKind::ApplicationManifest,
                scope: ContractScope {
                    id: format!("application/{project_name}"),
                },
                owner: format!("{project_name}/application"),
                identity: ArtifactIdentity::new(
                    ContractArtifactKind::ApplicationManifest,
                    format!("ref:application/{project_name}"),
                ),
                provenance: ArtifactProvenance {
                    sources,
                    generator: "distributed.application-manifest".to_string(),
                    source_revision: None,
                    glob_limit: None,
                },
                predecessor: None,
                outputs: BTreeMap::from([("manifest".to_string(), APPLICATION_OUTPUT.to_string())]),
                lifecycle: BTreeSet::from([
                    "build".to_string(),
                    "check".to_string(),
                    "dev".to_string(),
                ]),
                environment_policy: None,
            },
        )]),
    };

    let ui = root
        .join("ui")
        .join("package.json")
        .is_file()
        .then(|| PathBuf::from("ui"));
    let dev = lifecycle_dev(&runtime, ui.as_deref(), &executable, command_prefix);
    let describe_args = command_prefix
        .iter()
        .map(|arg| (*arg).to_string())
        .chain([
            "describe".to_string(),
            "--manifest-path".to_string(),
            "{root}/Cargo.toml".to_string(),
            "--package".to_string(),
            application.package.clone(),
            "--entrypoint".to_string(),
            entrypoint.clone(),
            "--distributed-path".to_string(),
            distributed_root.to_string_lossy().into_owned(),
        ])
        .collect();
    let config = LifecycleBuildConfig {
        schema_version: LIFECYCLE_BUILD_CONFIG_SCHEMA_VERSION,
        application: project_name.clone(),
        source: DistributedSourceIdentity {
            rust: source_identity.clone(),
            cli: source_identity.clone(),
            javascript: source_identity,
        },
        roots: BTreeSet::from([APPLICATION_NODE.to_string()]),
        executors: BTreeMap::from([(
            "distributed.application-manifest".to_string(),
            LifecycleExecutor {
                identity: executor_identity,
                program: executable.to_string_lossy().into_owned(),
                args: describe_args,
                stdout: Some(APPLICATION_OUTPUT.to_string()),
            },
        )]),
        dev: Some(dev),
    };
    config.validate()?;
    catalog
        .canonical_bytes()
        .map_err(|error| LifecycleError::new(error.to_string()))?;

    Ok(DiscoveredLifecycleProject {
        name: project_name,
        application_package: application.package,
        application_entrypoint: entrypoint,
        distributed_root,
        runtime_package: runtime.package,
        runtime_binary: runtime.binary,
        ui,
        plan: LifecycleProjectPlan {
            root,
            catalog,
            config,
            out: out
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from(".distributed/lifecycle")),
        },
    })
}

fn cargo_metadata(manifest: &Path) -> Result<CargoMetadata, LifecycleError> {
    let output = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--manifest-path"])
        .arg(manifest)
        .output()
        .map_err(|error| LifecycleError::new(format!("failed to run `cargo metadata`: {error}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(LifecycleError::new(format!(
            "`cargo metadata` failed for `{}`: {}",
            manifest.display(),
            stderr.trim()
        )));
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|error| LifecycleError::new(format!("failed to parse Cargo metadata: {error}")))
}

fn discover_application(
    workspace_metadata: &Value,
    packages: &[&CargoPackage],
) -> Result<ApplicationTarget, LifecycleError> {
    if let (Some(package), Some(entrypoint)) = (
        workspace_metadata
            .pointer("/distributed/application/package")
            .and_then(Value::as_str),
        workspace_metadata
            .pointer("/distributed/application/entrypoint")
            .and_then(Value::as_str),
    ) {
        let package = packages
            .iter()
            .find(|candidate| candidate.name == package)
            .ok_or_else(|| {
                LifecycleError::new(
                    "workspace Distributed application package is not a workspace member",
                )
            })?;
        return application_target(package, entrypoint);
    }

    let marked = packages
        .iter()
        .filter_map(|package| {
            package
                .metadata
                .pointer("/distributed/application/entrypoint")
                .and_then(Value::as_str)
                .map(|entrypoint| (*package, entrypoint))
        })
        .collect::<Vec<_>>();
    if marked.len() == 1 {
        return application_target(marked[0].0, marked[0].1);
    }
    if marked.len() > 1 {
        return Err(LifecycleError::new(
            "Cargo metadata marks more than one Distributed application target",
        ));
    }

    let conventional = packages
        .iter()
        .filter(|package| package.name.ends_with("-service"))
        .filter(|package| {
            package
                .targets
                .iter()
                .any(|target| target.kind.iter().any(|kind| kind == "lib"))
        })
        .copied()
        .collect::<Vec<_>>();
    if conventional.len() == 1 {
        return application_target(conventional[0], "application_manifest");
    }
    Err(LifecycleError::new(
        "unable to locate one typed Distributed application target in Cargo metadata; expose `application_manifest` from the application crate or mark its package metadata",
    ))
}

fn application_target(
    package: &CargoPackage,
    entrypoint: &str,
) -> Result<ApplicationTarget, LifecycleError> {
    let crate_name = package
        .targets
        .iter()
        .find(|target| target.kind.iter().any(|kind| kind == "lib"))
        .map(|target| target.name.clone())
        .ok_or_else(|| {
            LifecycleError::new(format!(
                "Distributed application package `{}` has no library target",
                package.name
            ))
        })?;
    Ok(ApplicationTarget {
        package: package.name.clone(),
        crate_name,
        entrypoint: entrypoint.to_string(),
    })
}

fn discover_runtime(
    workspace_metadata: &Value,
    packages: &[&CargoPackage],
) -> Result<RuntimeTarget, LifecycleError> {
    if let (Some(package), Some(binary)) = (
        workspace_metadata
            .pointer("/distributed/runtime/package")
            .and_then(Value::as_str),
        workspace_metadata
            .pointer("/distributed/runtime/binary")
            .and_then(Value::as_str),
    ) {
        return runtime_target(packages, package, binary);
    }
    let marked = packages
        .iter()
        .filter_map(|package| {
            package
                .metadata
                .pointer("/distributed/runtime/binary")
                .and_then(Value::as_str)
                .map(|binary| (package.name.as_str(), binary))
        })
        .collect::<Vec<_>>();
    if marked.len() == 1 {
        return runtime_target(packages, marked[0].0, marked[0].1);
    }
    if marked.len() > 1 {
        return Err(LifecycleError::new(
            "Cargo metadata marks more than one Distributed runtime target",
        ));
    }
    let binaries = packages
        .iter()
        .flat_map(|package| {
            package
                .targets
                .iter()
                .filter(|target| target.kind.iter().any(|kind| kind == "bin"))
                .filter(|target| !target.name.ends_with("-manifest"))
                .map(|target| (package.name.as_str(), target.name.as_str()))
        })
        .collect::<Vec<_>>();
    if binaries.len() == 1 {
        return runtime_target(packages, binaries[0].0, binaries[0].1);
    }
    Err(LifecycleError::new(
        "unable to locate one Distributed runtime binary in Cargo metadata",
    ))
}

fn runtime_target(
    packages: &[&CargoPackage],
    package_name: &str,
    binary: &str,
) -> Result<RuntimeTarget, LifecycleError> {
    let package = packages
        .iter()
        .find(|package| package.name == package_name)
        .ok_or_else(|| {
            LifecycleError::new("Distributed runtime package is not a workspace member")
        })?;
    if !package
        .targets
        .iter()
        .any(|target| target.name == binary && target.kind.iter().any(|kind| kind == "bin"))
    {
        return Err(LifecycleError::new(format!(
            "Distributed runtime package `{package_name}` has no `{binary}` binary target"
        )));
    }
    Ok(RuntimeTarget {
        package: package_name.to_string(),
        binary: binary.to_string(),
    })
}

fn workspace_sources(
    root: &Path,
    packages: &[&CargoPackage],
) -> Result<BTreeSet<String>, LifecycleError> {
    let mut sources = BTreeSet::from(["Cargo.toml".to_string()]);
    if root.join("Cargo.lock").is_file() {
        sources.insert("Cargo.lock".to_string());
    }
    for package in packages {
        let manifest = package.manifest_path.strip_prefix(root).map_err(|_| {
            LifecycleError::new(format!(
                "workspace package `{}` resolves outside the Cargo workspace",
                package.name
            ))
        })?;
        sources.insert(portable_path(manifest)?);
        let source = package
            .manifest_path
            .parent()
            .expect("Cargo manifest has a parent")
            .join("src");
        if source.is_dir() {
            let source = source.strip_prefix(root).map_err(|_| {
                LifecycleError::new("workspace source resolves outside the Cargo workspace")
            })?;
            sources.insert(portable_path(source)?);
        }
    }
    Ok(sources)
}

fn portable_path(path: &Path) -> Result<String, LifecycleError> {
    let value = path
        .components()
        .map(|component| component.as_os_str().to_str())
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| LifecycleError::new("project path is not valid UTF-8"))?
        .join("/");
    if value.is_empty() {
        return Err(LifecycleError::new("project path must not be empty"));
    }
    Ok(value)
}

fn source_identity(package: &CargoPackage, executable: &Path) -> Result<String, LifecycleError> {
    let manifest = fs::read(&package.manifest_path).map_err(|error| {
        LifecycleError::new(format!(
            "failed to read Distributed Cargo.toml `{}`: {error}",
            package.manifest_path.display()
        ))
    })?;
    let executable = fs::read(executable).map_err(|error| {
        LifecycleError::new(format!(
            "failed to read distributed executable `{}`: {error}",
            executable.display()
        ))
    })?;
    let mut bytes = Vec::with_capacity(manifest.len() + executable.len() + package.version.len());
    bytes.extend_from_slice(package.version.as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(&manifest);
    bytes.push(0);
    bytes.extend_from_slice(&executable);
    Ok(digest_bytes(&bytes))
}

fn lifecycle_dev(
    runtime: &RuntimeTarget,
    ui: Option<&Path>,
    executable: &Path,
    command_prefix: &[&str],
) -> LifecycleDevConfig {
    let bind = std::env::var("BIND").unwrap_or_else(|_| "127.0.0.1:8791".to_string());
    let api_address = loopback_address(&bind);
    let api_url = format!("http://{api_address}");
    let ui_host = std::env::var("UI_HOST").unwrap_or_else(|_| "localhost".to_string());
    let ui_port = std::env::var("UI_PORT").unwrap_or_else(|_| "5180".to_string());
    let ui_url = format!("http://{ui_host}:{ui_port}");
    let mut processes = BTreeMap::from([(
        "api".to_string(),
        LifecycleDevProcess {
            program: "cargo".to_string(),
            args: vec![
                "run".to_string(),
                "--package".to_string(),
                runtime.package.clone(),
                "--bin".to_string(),
                runtime.binary.clone(),
            ],
            cwd: None,
            env: BTreeMap::from([
                ("BIND".to_string(), bind.clone()),
                ("BIND_ADDR".to_string(), bind),
            ]),
            url: Some(api_url.clone()),
            restart_on: BTreeSet::from([APPLICATION_NODE.to_string()]),
            ready_after_ms: 100,
            ready: Some(LifecycleDevProbe {
                program: executable.to_string_lossy().into_owned(),
                args: probe_args(command_prefix, api_address),
                interval_ms: 250,
                timeout_ms: 15_000,
            }),
        },
    )]);
    if let Some(ui) = ui {
        processes.insert(
            "ui".to_string(),
            LifecycleDevProcess {
                program: "npm".to_string(),
                args: vec![
                    "run".to_string(),
                    "dev".to_string(),
                    "--".to_string(),
                    "--host".to_string(),
                    ui_host.clone(),
                    "--port".to_string(),
                    ui_port.clone(),
                ],
                cwd: Some(ui.to_string_lossy().into_owned()),
                env: BTreeMap::from([
                    ("DISTRIBUTED_API_ORIGIN".to_string(), api_url.clone()),
                    ("E2E_API_ORIGIN".to_string(), api_url.clone()),
                    (
                        "AUTH_SECRET".to_string(),
                        std::env::var("AUTH_SECRET").unwrap_or_else(|_| {
                            "distributed-local-development-only-secret".to_string()
                        }),
                    ),
                    ("AUTH_URL".to_string(), ui_url.clone()),
                    ("AUTH_USE_SECURE_COOKIES".to_string(), "false".to_string()),
                ]),
                url: Some(ui_url.clone()),
                restart_on: BTreeSet::new(),
                ready_after_ms: 100,
                ready: Some(LifecycleDevProbe {
                    program: executable.to_string_lossy().into_owned(),
                    args: probe_args(command_prefix, socket_address(&ui_host, &ui_port)),
                    interval_ms: 250,
                    timeout_ms: 15_000,
                }),
            },
        );
    }
    LifecycleDevConfig {
        poll_ms: 500,
        debounce_ms: 250,
        shutdown_ms: 5_000,
        processes,
    }
}

fn probe_args(command_prefix: &[&str], address: String) -> Vec<String> {
    command_prefix
        .iter()
        .map(|arg| (*arg).to_string())
        .chain(["__probe".to_string(), "--address".to_string(), address])
        .collect()
}

fn loopback_address(bind: &str) -> String {
    let Some((host, port)) = bind.rsplit_once(':') else {
        return bind.to_string();
    };
    let host = match host.trim_matches(['[', ']']) {
        "0.0.0.0" | "::" => "127.0.0.1",
        host => host,
    };
    socket_address(host, port)
}

fn socket_address(host: &str, port: &str) -> String {
    if host.contains(':') && !(host.starts_with('[') && host.ends_with(']')) {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package(name: &str, metadata: Value, targets: &[(&str, &str)]) -> CargoPackage {
        CargoPackage {
            id: format!("path+file:///fixture#{name}@0.1.0"),
            name: name.to_string(),
            version: "0.1.0".to_string(),
            manifest_path: PathBuf::from(format!("/fixture/{name}/Cargo.toml")),
            metadata,
            targets: targets
                .iter()
                .map(|(name, kind)| CargoTarget {
                    name: (*name).to_string(),
                    kind: vec![(*kind).to_string()],
                })
                .collect(),
        }
    }

    #[test]
    fn package_metadata_selects_typed_application_and_runtime() {
        let service = package(
            "orders-core",
            serde_json::json!({
                "distributed": { "application": { "entrypoint": "orders_application" } }
            }),
            &[("orders_core", "lib")],
        );
        let runner = package(
            "orders-host",
            serde_json::json!({
                "distributed": { "runtime": { "binary": "orders" } }
            }),
            &[("orders", "bin")],
        );
        let packages = vec![&service, &runner];

        let application = discover_application(&Value::Null, &packages).unwrap();
        let runtime = discover_runtime(&Value::Null, &packages).unwrap();

        assert_eq!(application.package, "orders-core");
        assert_eq!(application.crate_name, "orders_core");
        assert_eq!(application.entrypoint, "orders_application");
        assert_eq!(runtime.package, "orders-host");
        assert_eq!(runtime.binary, "orders");
    }

    #[test]
    fn conventional_workspace_needs_no_metadata() {
        let service = package("orders-service", Value::Null, &[("orders_service", "lib")]);
        let runner = package(
            "orders-runner",
            Value::Null,
            &[("orders", "bin"), ("orders-manifest", "bin")],
        );
        let packages = vec![&service, &runner];

        let application = discover_application(&Value::Null, &packages).unwrap();
        let runtime = discover_runtime(&Value::Null, &packages).unwrap();

        assert_eq!(application.entrypoint, "application_manifest");
        assert_eq!(runtime.binary, "orders");
    }

    #[test]
    fn readiness_addresses_are_valid_for_wildcard_and_ipv6_binds() {
        assert_eq!(loopback_address("0.0.0.0:8791"), "127.0.0.1:8791");
        assert_eq!(loopback_address("[::]:8791"), "127.0.0.1:8791");
        assert_eq!(loopback_address("[::1]:8791"), "[::1]:8791");
        assert_eq!(socket_address("localhost", "5180"), "localhost:5180");
        assert_eq!(
            probe_args(&["service"], "localhost:5180".to_string()),
            ["service", "__probe", "--address", "localhost:5180"]
        );
    }
}
