//! Integration test for `dsvc scaffold`: drive the real binary and assert the
//! generated project tree. Pure generation + filesystem, so it is fast and needs
//! no nested compilation.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Repo root (the `distributed` crate) — `distributed_cli`'s parent.
fn distributed_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("distributed_cli has a parent directory")
        .to_path_buf()
}

#[test]
fn scaffold_generates_a_service_tree() {
    let out_dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("scaffold-orders");
    let _ = fs::remove_dir_all(&out_dir);

    let status = Command::new(env!("CARGO_BIN_EXE_dsvc"))
        .args([
            "scaffold",
            "orders",
            "--path",
            out_dir.to_str().unwrap(),
            "--store",
            "postgres",
            "--gitops",
            "--distributed-path",
            distributed_root().to_str().unwrap(),
        ])
        .status()
        .expect("dsvc should run");
    assert!(status.success(), "dsvc scaffold failed");

    for expected in [
        "Cargo.toml",
        "src/lib.rs",
        "src/main.rs",
        "src/service.rs",
        ".gitops/deploy/Chart.yaml",
    ] {
        assert!(
            out_dir.join(expected).exists(),
            "missing generated file: {expected}"
        );
    }

    let cargo = fs::read_to_string(out_dir.join("Cargo.toml")).unwrap();
    assert!(cargo.contains("\"postgres\""), "Cargo.toml: {cargo}");
}
