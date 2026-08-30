//! Compilation of browser-side Rust pure functions declared by command contracts.
//!
//! The declaration is application source. The CLI owns turning that declaration
//! into the JavaScript and WebAssembly files imported by generated clients.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

const MAX_SOURCE_FILES: usize = 20_000;
const MAX_SOURCE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_PURES: usize = 64;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct WasmPure {
    rust_package: String,
    import: String,
}

#[derive(Debug, Deserialize)]
struct CargoMetadata {
    workspace_root: PathBuf,
    workspace_members: Vec<String>,
    packages: Vec<CargoPackage>,
}

#[derive(Debug, Deserialize)]
struct CargoPackage {
    id: String,
    name: String,
    manifest_path: PathBuf,
    source: Option<String>,
    #[serde(default)]
    features: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct WasmStamp {
    schema_version: u64,
    source_identity: String,
    rust_package: String,
    import: String,
}

pub(crate) fn build_declared_wasm_pures(
    manifest: &Value,
    project_root: &Path,
) -> Result<usize, Box<dyn Error>> {
    let ui_lib = project_root.join("ui/src/lib");
    if !project_root.join("ui/package.json").is_file() {
        return Ok(0);
    }
    let pures = collect_wasm_pures(manifest)?;
    if pures.is_empty() {
        return Ok(0);
    }
    let metadata = cargo_metadata(&project_root.join("Cargo.toml"))?;
    let source_identity = workspace_source_identity(&metadata)?;
    let workspace_ids = metadata
        .workspace_members
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let packages = metadata
        .packages
        .iter()
        .filter(|package| workspace_ids.contains(package.id.as_str()))
        .map(|package| (package.name.as_str(), package))
        .collect::<BTreeMap<_, _>>();

    let mut outputs = BTreeMap::<PathBuf, &WasmPure>::new();
    for pure in &pures {
        let relative = portable_import_path(&pure.import)?;
        let destination = ui_lib.join(relative.parent().ok_or_else(|| {
            format!(
                "WASM import `{}` must include an output directory",
                pure.import
            )
        })?);
        if let Some(existing) = outputs.insert(destination.clone(), pure) {
            if existing != pure {
                return Err(format!(
                    "WASM pures `{}` and `{}` claim the same output directory `{}`",
                    existing.import,
                    pure.import,
                    destination.display()
                )
                .into());
            }
        }
    }

    for (destination, pure) in outputs {
        let package = packages.get(pure.rust_package.as_str()).ok_or_else(|| {
            format!(
                "WASM pure `{}` is declared by Cargo package `{}`, which is not a workspace member",
                pure.import, pure.rust_package
            )
        })?;
        if !package.features.contains_key("wasm") {
            return Err(format!(
                "WASM pure `{}` requires package `{}` to define a `wasm` Cargo feature",
                pure.import, pure.rust_package
            )
            .into());
        }
        let output_name = Path::new(&pure.import)
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| format!("WASM import `{}` has no UTF-8 output name", pure.import))?;
        let stamp = WasmStamp {
            schema_version: 1,
            source_identity: source_identity.clone(),
            rust_package: pure.rust_package.clone(),
            import: pure.import.clone(),
        };
        let stamp_name = format!("{:x}.json", Sha256::digest(pure.import.as_bytes()));
        let stamp_path = project_root.join(".distributed/wasm").join(stamp_name);
        if stamp_matches(&stamp_path, &destination, &stamp)? {
            eprintln!("distributed: browser WASM {} is current", pure.import);
            continue;
        }
        let package_dir = package
            .manifest_path
            .parent()
            .ok_or_else(|| format!("package `{}` Cargo.toml has no parent", package.name))?;
        build_wasm_pure(
            project_root,
            package_dir,
            output_name,
            &destination,
            &stamp_path,
            &stamp,
        )?;
    }
    Ok(pures.len())
}

fn collect_wasm_pures(manifest: &Value) -> Result<BTreeSet<WasmPure>, Box<dyn Error>> {
    fn visit(value: &Value, found: &mut BTreeSet<WasmPure>) -> Result<(), Box<dyn Error>> {
        match value {
            Value::Array(values) => {
                for value in values {
                    visit(value, found)?;
                }
            }
            Value::Object(object) => {
                if let Some(import) = object.get("wasm_package").and_then(Value::as_str) {
                    if !import.is_empty() {
                        let rust_package = object
                            .get("wasm_rust_package")
                            .and_then(Value::as_str)
                            .filter(|package| !package.is_empty())
                            .ok_or_else(|| {
                                format!(
                                    "WASM pure `{import}` does not identify its declaring Cargo package; declare it through `portable_command!`"
                                )
                            })?;
                        found.insert(WasmPure {
                            rust_package: rust_package.to_string(),
                            import: import.to_string(),
                        });
                        if found.len() > MAX_PURES {
                            return Err(format!(
                                "application declares more than {MAX_PURES} browser WASM pures"
                            )
                            .into());
                        }
                    }
                }
                for value in object.values() {
                    visit(value, found)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    let mut found = BTreeSet::new();
    visit(manifest, &mut found)?;
    Ok(found)
}

fn portable_import_path(import: &str) -> Result<PathBuf, Box<dyn Error>> {
    if import.contains('\\') || import.contains('\0') {
        return Err(format!("WASM import `{import}` is not a portable relative path").into());
    }
    let path = Path::new(import);
    if path.is_absolute()
        || path.components().count() < 2
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(format!("WASM import `{import}` is not a portable relative path").into());
    }
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if name.is_empty()
        || !name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
    {
        return Err(format!("WASM import `{import}` has an invalid output name").into());
    }
    Ok(path.to_path_buf())
}

fn cargo_metadata(manifest: &Path) -> Result<CargoMetadata, Box<dyn Error>> {
    let output = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--manifest-path"])
        .arg(manifest)
        .output()
        .map_err(|error| format!("failed to run `cargo metadata` for browser WASM: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "`cargo metadata` failed while resolving browser WASM packages: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    Ok(serde_json::from_slice(&output.stdout)?)
}

fn workspace_source_identity(metadata: &CargoMetadata) -> Result<String, Box<dyn Error>> {
    let mut files = BTreeSet::new();
    for path in [
        metadata.workspace_root.join("Cargo.toml"),
        metadata.workspace_root.join("Cargo.lock"),
    ] {
        if path.is_file() {
            files.insert(path);
        }
    }
    for package in metadata
        .packages
        .iter()
        .filter(|package| package.source.is_none())
    {
        files.insert(package.manifest_path.clone());
        let package_dir = package
            .manifest_path
            .parent()
            .ok_or_else(|| format!("package `{}` Cargo.toml has no parent", package.name))?;
        collect_source_files(package_dir, &mut files)?;
    }
    let mut hash = Sha256::new();
    let mut bytes = 0_u64;
    for file in files {
        let content = fs::read(&file)?;
        bytes = bytes.saturating_add(content.len() as u64);
        if bytes > MAX_SOURCE_BYTES {
            return Err(format!(
                "Rust workspace sources exceed the {MAX_SOURCE_BYTES}-byte WASM fingerprint limit"
            )
            .into());
        }
        hash.update(file.to_string_lossy().as_bytes());
        hash.update([0]);
        hash.update(&content);
        hash.update([0]);
    }
    Ok(format!("sha256:{:x}", hash.finalize()))
}

fn collect_source_files(
    directory: &Path,
    files: &mut BTreeSet<PathBuf>,
) -> Result<(), Box<dyn Error>> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let name = entry.file_name();
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            if matches!(
                name.to_str(),
                Some("target" | "node_modules" | "ui" | ".git" | ".distributed" | ".worktrees")
            ) {
                continue;
            }
            collect_source_files(&entry.path(), files)?;
        } else if file_type.is_file() {
            let path = entry.path();
            let include = matches!(
                path.extension().and_then(|ext| ext.to_str()),
                Some("rs" | "toml" | "lock")
            );
            if include {
                files.insert(path);
                if files.len() > MAX_SOURCE_FILES {
                    return Err(format!(
                        "Rust workspace contains more than {MAX_SOURCE_FILES} source files for WASM fingerprinting"
                    )
                    .into());
                }
            }
        }
    }
    Ok(())
}

fn stamp_matches(
    stamp_path: &Path,
    destination: &Path,
    expected: &WasmStamp,
) -> Result<bool, Box<dyn Error>> {
    let Ok(bytes) = fs::read(stamp_path) else {
        return Ok(false);
    };
    let Ok(observed) = serde_json::from_slice::<WasmStamp>(&bytes) else {
        return Ok(false);
    };
    Ok(observed.schema_version == expected.schema_version
        && observed.source_identity == expected.source_identity
        && observed.rust_package == expected.rust_package
        && observed.import == expected.import
        && destination
            .join(format!(
                "{}_bg.wasm",
                Path::new(&expected.import)
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
            ))
            .is_file()
        && destination
            .join(format!(
                "{}.js",
                Path::new(&expected.import)
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
            ))
            .is_file())
}

fn build_wasm_pure(
    project_root: &Path,
    package_dir: &Path,
    output_name: &str,
    destination: &Path,
    stamp_path: &Path,
    stamp: &WasmStamp,
) -> Result<(), Box<dyn Error>> {
    let parent = destination
        .parent()
        .ok_or("WASM output has no parent directory")?;
    ensure_real_directory_path(project_root, parent)?;
    if let Ok(metadata) = fs::symlink_metadata(destination) {
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(format!(
                "WASM output `{}` must be a real directory",
                destination.display()
            )
            .into());
        }
    }
    let stage = tempfile::Builder::new()
        .prefix(".distributed-wasm-stage-")
        .tempdir_in(parent)?;
    eprintln!(
        "distributed: compiling required browser WASM {} from Cargo package {}",
        stamp.import, stamp.rust_package
    );
    let output = Command::new("wasm-pack")
        .arg("build")
        .arg(package_dir)
        .args(["--target", "web", "--out-dir"])
        .arg(stage.path())
        .args(["--out-name", output_name, "--", "--no-default-features", "--features", "wasm"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::inherit())
        .status()
        .map_err(|error| {
            format!(
                "failed to start `wasm-pack` for required WASM pure `{}`; install wasm-pack and ensure it is on PATH: {error}",
                stamp.import
            )
        })?;
    if !output.success() {
        return Err(format!(
            "required WASM pure `{}` failed to compile with {output}",
            stamp.import
        )
        .into());
    }
    for required in [
        stage.path().join(format!("{output_name}.js")),
        stage.path().join(format!("{output_name}_bg.wasm")),
    ] {
        if !required.is_file() {
            return Err(format!(
                "wasm-pack succeeded for `{}` but did not produce `{}`",
                stamp.import,
                required.file_name().unwrap().to_string_lossy()
            )
            .into());
        }
    }
    let staged_path = stage.keep();
    let backup = tempfile::Builder::new()
        .prefix(".distributed-wasm-backup-")
        .tempdir_in(parent)?;
    let backup_path = backup.path().to_path_buf();
    backup.close()?;
    let had_destination = destination.exists();
    if had_destination {
        fs::rename(destination, &backup_path)?;
    }
    if let Err(error) = fs::rename(&staged_path, destination) {
        if had_destination {
            let _ = fs::rename(&backup_path, destination);
        }
        let _ = fs::remove_dir_all(&staged_path);
        return Err(format!(
            "failed to activate required WASM output `{}`: {error}",
            destination.display()
        )
        .into());
    }
    if had_destination {
        fs::remove_dir_all(backup_path)?;
    }
    if let Some(parent) = stamp_path.parent() {
        ensure_real_directory_path(project_root, parent)?;
    }
    fs::write(stamp_path, serde_json::to_vec(stamp)?)?;
    Ok(())
}

fn ensure_real_directory_path(root: &Path, directory: &Path) -> Result<(), Box<dyn Error>> {
    let relative = directory.strip_prefix(root).map_err(|_| {
        format!(
            "generated directory `{}` escapes project `{}`",
            directory.display(),
            root.display()
        )
    })?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        if !matches!(component, Component::Normal(_)) {
            return Err(format!(
                "generated directory `{}` is not a portable project path",
                directory.display()
            )
            .into());
        }
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(format!(
                    "generated directory component `{}` must be a real directory",
                    current.display()
                )
                .into())
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&current)?;
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn collects_declared_wasm_pures_recursively() {
        let manifest = json!({"commands": [{"pure_reduces": [{
            "wasm_package": "blob/pkg/blob_wasm",
            "wasm_rust_package": "blob-domain"
        }]}]});
        let found = collect_wasm_pures(&manifest).unwrap();
        assert!(found.contains(&WasmPure {
            rust_package: "blob-domain".to_string(),
            import: "blob/pkg/blob_wasm".to_string(),
        }));
    }

    #[test]
    fn rejects_declared_wasm_without_a_rust_package() {
        let error = collect_wasm_pures(&json!({"wasm_package": "blob/pkg/blob_wasm"})).unwrap_err();
        assert!(error.to_string().contains("declaring Cargo package"));
    }

    #[test]
    fn rejects_import_path_traversal() {
        let error = portable_import_path("../outside/module").unwrap_err();
        assert!(error.to_string().contains("portable relative path"));
    }
}
