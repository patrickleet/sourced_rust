//! End-to-end tests for the manifest-harness commands (`describe` / `schema`).
//! These compile the `orders-service` fixture via a nested `cargo` build, so they
//! are `#[ignore]`d by default and run explicitly in the integration CI job
//! (`cargo test -p distributed_cli --include-ignored`).

use std::path::{Path, PathBuf};
use std::process::Command;

fn distributed_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("distributed_cli has a parent directory")
        .to_path_buf()
}

fn fixture_manifest() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/orders-service/Cargo.toml")
}

/// Run `dsvc <args...>` against the fixture, returning stdout. Always passes
/// `--manifest-path` and `--distributed-path` so resolution is deterministic.
fn dsvc(args: &[&str]) -> String {
    let root = distributed_root();
    let manifest = fixture_manifest();
    let output = Command::new(env!("CARGO_BIN_EXE_dsvc"))
        .args(args)
        .args(["--manifest-path", manifest.to_str().unwrap()])
        .args(["--distributed-path", root.to_str().unwrap()])
        .output()
        .expect("dsvc should run");
    assert!(
        output.status.success(),
        "dsvc {args:?} failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
#[ignore = "compiles the fixture via the manifest harness; run in the integration job"]
fn describe_emits_manifest_json() {
    let json = dsvc(&["describe"]);
    assert!(json.contains("\"schema_version\""), "json: {json}");
    assert!(json.contains("\"orders\""), "json: {json}");
}

#[test]
#[ignore = "compiles the fixture via the manifest harness; run in the integration job"]
fn schema_renders_postgres_sql() {
    let sql = dsvc(&["schema", "--dialect", "postgres"]);
    assert!(sql.contains("CREATE TABLE"), "sql: {sql}");
    assert!(sql.contains("orders"), "sql: {sql}");
}

#[test]
#[ignore = "compiles the fixture via the manifest harness; run in the integration job"]
fn schema_renders_atlas_resource() {
    let yaml = dsvc(&[
        "schema",
        "--format",
        "atlas",
        "--name",
        "orders",
        "--db-secret",
        "orders-db",
    ]);
    assert!(yaml.contains("kind: AtlasSchema"), "yaml: {yaml}");
    assert!(yaml.contains("secretKeyRef"), "yaml: {yaml}");
    assert!(yaml.contains("CREATE TABLE"), "yaml: {yaml}");
}
