//! The manifest harness: `describe`/`schema` compile a tiny generated crate
//! that depends on the target service, calls its `distributed_manifest()`
//! entrypoint, and prints the manifest JSON or rendered SQL. This module owns
//! that codegen and the nested `cargo` invocations; the `cli` module maps
//! command flags onto [`HarnessOptions`]/[`HarnessMode`].

use serde::Deserialize;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::cli::{path_for_toml, resolve_distributed_path};
use crate::SchemaDialect;

#[derive(Clone, Debug)]
pub(crate) struct HarnessOptions {
    pub(crate) path: PathBuf,
    pub(crate) manifest_path: Option<PathBuf>,
    pub(crate) package: Option<String>,
    pub(crate) features: Vec<String>,
    pub(crate) no_default_features: bool,
    pub(crate) entrypoint: Option<String>,
    pub(crate) distributed_path: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum HarnessMode {
    DescribeJson,
    SchemaSql(SchemaDialect),
    SchemaGraphql,
}

impl HarnessMode {
    fn cache_key(self) -> &'static str {
        match self {
            HarnessMode::DescribeJson => "describe-json",
            HarnessMode::SchemaSql(SchemaDialect::Postgres) => "schema-postgres",
            HarnessMode::SchemaSql(SchemaDialect::Sqlite) => "schema-sqlite",
            HarnessMode::SchemaGraphql => "schema-graphql",
        }
    }
}

pub(crate) fn run_manifest_harness(
    options: &HarnessOptions,
    mode: HarnessMode,
) -> Result<String, Box<dyn Error>> {
    let manifest_path =
        resolve_target_manifest_path(&options.path, options.manifest_path.as_deref())?;
    let package = cargo_package(&manifest_path, options.package.as_deref())?;
    let distributed_path =
        resolve_distributed_path(options.distributed_path.as_deref(), &package.directory)?;
    let crate_ident = package.name.replace('-', "_");
    let entrypoint = options
        .entrypoint
        .clone()
        .map(|entrypoint| qualify_entrypoint(&crate_ident, &entrypoint))
        .unwrap_or_else(|| Ok(format!("{crate_ident}::distributed_manifest")))?;
    validate_rust_path(&entrypoint)?;

    let harness_root = package.directory.join("target/dctl-manifest-harness");
    let harness_dir = harness_root.join(mode.cache_key());
    fs::create_dir_all(harness_dir.join("src"))?;
    fs::write(
        harness_dir.join("Cargo.toml"),
        harness_cargo_toml(
            &format!("dctl-manifest-harness-{}", mode.cache_key()),
            &crate_ident,
            &package.name,
            &package.directory,
            &distributed_path,
            &options.features,
            options.no_default_features,
        ),
    )?;
    fs::write(
        harness_dir.join("src/main.rs"),
        harness_main_rs(&entrypoint, mode),
    )?;

    let manifest_path = harness_dir.join("Cargo.toml");
    let output = Command::new("cargo")
        .args([
            "run",
            "--quiet",
            "--manifest-path",
            manifest_path.to_string_lossy().as_ref(),
        ])
        .env("CARGO_TARGET_DIR", harness_root.join("target"))
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("manifest harness failed: {stderr}").into());
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn harness_cargo_toml(
    harness_package_name: &str,
    crate_ident: &str,
    package_name: &str,
    package_dir: &Path,
    distributed_path: &Path,
    features: &[String],
    no_default_features: bool,
) -> String {
    let features = features
        .iter()
        .map(toml_string)
        .collect::<Vec<_>>()
        .join(", ");
    let default_features = if no_default_features {
        ", default-features = false"
    } else {
        ""
    };

    format!(
        r#"[package]
name = {harness_package_name}
version = "0.1.0"
edition = "2021"

[workspace]

[dependencies]
distributed = {{ path = {distributed_path} }}
serde_json = "1"
{crate_ident} = {{ package = {package_name}, path = {package_dir}{default_features}, features = [{features}] }}
"#,
        harness_package_name = toml_string(harness_package_name),
        distributed_path = toml_string(path_for_toml(distributed_path)),
        package_name = toml_string(package_name),
        package_dir = toml_string(path_for_toml(package_dir)),
    )
}

fn harness_main_rs(entrypoint: &str, mode: HarnessMode) -> String {
    match mode {
        HarnessMode::DescribeJson => format!(
            r#"fn main() {{
    let manifest = {entrypoint}();
    let envelope = distributed::DistributedManifestEnvelope::new(manifest);
    println!("{{}}", serde_json::to_string_pretty(&envelope).expect("manifest should serialize"));
}}
"#
        ),
        HarnessMode::SchemaSql(dialect) => {
            let dialect = match dialect {
                SchemaDialect::Postgres => "Postgres",
                SchemaDialect::Sqlite => "Sqlite",
            };
            format!(
                r#"fn main() {{
    let manifest = {entrypoint}();
    let envelope = distributed::DistributedManifestEnvelope::new(manifest);
    let statements = envelope
        .project
        .sql_statements(distributed::table::TableSqlDialect::{dialect})
        .expect("manifest SQL should render");
    if !statements.is_empty() {{
        println!("{{}}", statements.join("\n\n"));
    }}
}}
"#
            )
        }
        HarnessMode::SchemaGraphql => format!(
            r#"fn main() {{
    let manifest = {entrypoint}();
    let envelope = distributed::DistributedManifestEnvelope::new(manifest);
    let sdl = envelope
        .project
        .graphql_sdl()
        .expect("manifest GraphQL SDL should render");
    print!("{{}}", sdl);
}}
"#
        ),
    }
}

fn resolve_target_manifest_path(
    path: &Path,
    manifest_path: Option<&Path>,
) -> Result<PathBuf, Box<dyn Error>> {
    let manifest = if let Some(manifest_path) = manifest_path {
        manifest_path.to_path_buf()
    } else if path.is_dir() {
        path.join("Cargo.toml")
    } else {
        path.to_path_buf()
    };

    if !manifest.exists() {
        return Err(format!("target manifest not found: {}", manifest.display()).into());
    }
    Ok(manifest.canonicalize()?)
}

#[derive(Clone, Debug)]
struct CargoPackage {
    name: String,
    directory: PathBuf,
}

fn cargo_package(
    manifest_path: &Path,
    package_name: Option<&str>,
) -> Result<CargoPackage, Box<dyn Error>> {
    let output = Command::new("cargo")
        .args([
            "metadata",
            "--no-deps",
            "--format-version",
            "1",
            "--manifest-path",
            manifest_path.to_string_lossy().as_ref(),
        ])
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("cargo metadata failed: {stderr}").into());
    }

    let metadata: CargoMetadata = serde_json::from_slice(&output.stdout)?;
    let selected = if let Some(package_name) = package_name {
        metadata
            .packages
            .into_iter()
            .find(|package| package.name == package_name)
            .ok_or_else(|| format!("package `{package_name}` was not found in cargo metadata"))?
    } else if metadata.packages.len() == 1 {
        metadata
            .packages
            .into_iter()
            .next()
            .expect("single package should exist")
    } else {
        let manifest_path = manifest_path.canonicalize()?;
        metadata
            .packages
            .into_iter()
            .find(|package| {
                Path::new(&package.manifest_path).canonicalize().ok() == Some(manifest_path.clone())
            })
            .ok_or("multiple packages found; pass --package to select one")?
    };
    let manifest_path = PathBuf::from(&selected.manifest_path);
    let directory = manifest_path
        .parent()
        .ok_or("cargo package manifest has no parent directory")?
        .to_path_buf();

    Ok(CargoPackage {
        name: selected.name,
        directory,
    })
}

#[derive(Debug, Deserialize)]
struct CargoMetadata {
    packages: Vec<CargoMetadataPackage>,
}

#[derive(Debug, Deserialize)]
struct CargoMetadataPackage {
    name: String,
    manifest_path: String,
}

fn qualify_entrypoint(crate_ident: &str, entrypoint: &str) -> Result<String, Box<dyn Error>> {
    let entrypoint = entrypoint.trim();
    if entrypoint.is_empty() {
        return Err("entrypoint cannot be empty".into());
    }
    if entrypoint.contains("::") {
        Ok(entrypoint.to_string())
    } else {
        Ok(format!("{crate_ident}::{entrypoint}"))
    }
}

fn validate_rust_path(path: &str) -> Result<(), Box<dyn Error>> {
    let valid = path
        .split("::")
        .all(|segment| !segment.is_empty() && is_rust_ident(segment));
    if valid {
        Ok(())
    } else {
        Err(format!("invalid Rust entrypoint path `{path}`").into())
    }
}

fn is_rust_ident(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|char| char == '_' || char.is_ascii_alphanumeric())
}

fn toml_string(value: impl AsRef<str>) -> String {
    serde_json::to_string(value.as_ref()).expect("string serialization should succeed")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn harness_is_standalone_inside_cached_target_directory() {
        let cargo_toml = harness_cargo_toml(
            "dctl-manifest-harness-schema-postgres",
            "todo_model",
            "todo-model",
            Path::new("/tmp/todo-model"),
            Path::new("/tmp/distributed"),
            &[],
            false,
        );

        assert!(cargo_toml.contains("\n[workspace]\n"));
        assert!(cargo_toml.contains("name = \"dctl-manifest-harness-schema-postgres\""));
    }

    #[test]
    fn schema_harness_uses_public_table_module_sql_dialect() {
        let main_rs = harness_main_rs(
            "orders_service::distributed_manifest",
            HarnessMode::SchemaSql(SchemaDialect::Postgres),
        );

        assert!(
            main_rs.contains("distributed::table::TableSqlDialect::Postgres"),
            "main.rs: {main_rs}"
        );
        assert!(
            !main_rs.contains("distributed::TableSqlDialect"),
            "main.rs: {main_rs}"
        );
    }
}
