//! Process-level coherent lifecycle build checks.

use distributed_cli::{run_lifecycle_dev, LifecycleBuildOptions, LifecycleDevOptions};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

fn distributed(root: &Path, extra: &[&str]) -> Output {
    let mut args = vec![
        "build",
        "--root",
        root.to_str().expect("UTF-8 fixture root"),
        "--output",
        "json",
    ];
    args.extend_from_slice(extra);
    Command::new(env!("CARGO_BIN_EXE_distributed"))
        .args(args)
        .output()
        .expect("distributed build should run")
}

fn temporary_root(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock after epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("distributed-lifecycle-{label}-{nanos}"));
    fs::create_dir_all(root.join("src")).expect("create lifecycle fixture root");
    root
}

fn write_fixture(root: &Path) {
    fs::write(root.join("src/input.txt"), "first\n").expect("write lifecycle input");
    fs::write(
        root.join("build-app.sh"),
        r#"#!/bin/sh
set -eu
root="$1"
stage="$2"
sed 's/^/application:/' "$root/src/input.txt"
if test -f "$root/produce-unowned"; then
  printf 'unowned\n' > "$stage/unowned.txt"
fi
"#,
    )
    .expect("write application executor");
    fs::write(
        root.join("build-plan.sh"),
        r#"#!/bin/sh
set -eu
root="$1"
stage="$2"
if test -f "$root/fail-plan"; then
  printf 'injected downstream failure\n' >&2
  exit 17
fi
sed 's/^/plan:/' "$stage/generated/application.json" > "$stage/generated/plan.json"
"#,
    )
    .expect("write plan executor");
    fs::create_dir_all(root.join("generated")).expect("create tracked output directory");
    fs::write(
        root.join("generated/application.json"),
        "application:first\n",
    )
    .expect("write accepted application output");
    fs::write(root.join("generated/plan.json"), "plan:application:first\n")
        .expect("write accepted plan output");

    let catalog = serde_json::json!({
        "schema_version": 1,
        "entries": {
            "application": {
                "id": "application",
                "kind": "application_manifest",
                "scope": { "id": "application/fixture" },
                "owner": "application/fixture",
                "identity": { "kind": "application_manifest", "value": "ref:application" },
                "provenance": {
                    "sources": ["src/input.txt"],
                    "generator": "fixture.application"
                },
                "outputs": { "manifest": "generated/application.json" },
                "lifecycle": ["build", "check", "dev"]
            },
            "plan": {
                "id": "plan",
                "kind": "deployment_plan",
                "scope": { "id": "deployment/fixture" },
                "owner": "deployment/fixture",
                "identity": { "kind": "deployment_plan", "value": "ref:plan" },
                "provenance": {
                    "sources": ["generated/application.json"],
                    "generator": "fixture.plan"
                },
                "predecessor": {
                    "entry_id": "application",
                    "identity": { "kind": "application_manifest", "value": "ref:application" }
                },
                "outputs": { "plan": "generated/plan.json" },
                "lifecycle": ["build", "check", "dev"]
            }
        }
    });
    fs::write(
        root.join("distributed.contracts.json"),
        serde_json::to_vec_pretty(&catalog).expect("serialize lifecycle catalog"),
    )
    .expect("write lifecycle catalog");
    let identity = format!("sha256:{}", "1".repeat(64));
    let source_identity = format!("sha256:{}", "a".repeat(64));
    let config = serde_json::json!({
        "schema_version": 1,
        "application": "fixture",
        "source": {
            "rust": source_identity,
            "cli": format!("sha256:{}", "a".repeat(64)),
            "javascript": format!("sha256:{}", "a".repeat(64))
        },
        "roots": ["plan"],
        "executors": {
            "fixture.application": {
                "identity": identity,
                "program": "/bin/sh",
                "args": ["{root}/build-app.sh", "{root}", "{stage}"],
                "stdout": "generated/application.json"
            },
            "fixture.plan": {
                "identity": format!("sha256:{}", "2".repeat(64)),
                "program": "/bin/sh",
                "args": ["{root}/build-plan.sh", "{root}", "{stage}"]
            }
        }
    });
    fs::write(
        root.join("distributed.lifecycle.json"),
        serde_json::to_vec_pretty(&config).expect("serialize lifecycle config"),
    )
    .expect("write lifecycle config");
}

fn report(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "parse lifecycle report: {error}; stdout={}; stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn enable_dev(root: &Path) {
    fs::write(
        root.join("dev-child.sh"),
        r#"#!/bin/sh
set -eu
root="$1"
name="$2"
if test -f "$root/dist/distributed/active.json"; then
  barrier=present
else
  barrier=missing
fi
printf '%s:%s:%s\n' "$name" "$barrier" "$DISTRIBUTED_GENERATION_ID" >> "$root/dev-process.log"
exec tail -f /dev/null
"#,
    )
    .expect("write dev child fixture");
    let path = root.join("distributed.lifecycle.json");
    let mut config: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    config["dev"] = serde_json::json!({
        "poll_ms": 20,
        "debounce_ms": 30,
        "shutdown_ms": 1000,
        "processes": {
            "api": {
                "program": "/bin/sh",
                "args": ["{root}/dev-child.sh", "{root}", "{process}"],
                "restart_on": ["application"],
                "ready_after_ms": 20
            },
            "ui": {
                "program": "/bin/sh",
                "args": ["{root}/dev-child.sh", "{root}", "{process}"],
                "restart_on": [],
                "ready_after_ms": 20
            }
        }
    });
    fs::write(path, serde_json::to_vec_pretty(&config).unwrap()).unwrap();
}

fn wait_until(timeout: Duration, predicate: impl Fn() -> bool) {
    let started = std::time::Instant::now();
    while !predicate() {
        assert!(
            started.elapsed() < timeout,
            "timed out waiting for fixture state"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn file_snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, path: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
        let mut entries = fs::read_dir(path)
            .expect("read snapshot directory")
            .collect::<Result<Vec<_>, _>>()
            .expect("read snapshot entries");
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let kind = entry.file_type().expect("inspect snapshot entry");
            if kind.is_dir() {
                visit(root, &entry.path(), files);
            } else if kind.is_file() {
                files.insert(
                    entry.path().strip_prefix(root).unwrap().to_path_buf(),
                    fs::read(entry.path()).expect("read snapshot file"),
                );
            }
        }
    }
    let mut files = BTreeMap::new();
    visit(root, root, &mut files);
    files
}

#[test]
fn build_is_deterministic_and_check_is_read_only_and_content_based() {
    let root = temporary_root("deterministic");
    write_fixture(&root);

    let first = distributed(&root, &[]);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let first = report(&first);
    let active_before = fs::read(root.join("dist/distributed/active.json"))
        .expect("read active generation pointer");
    let generation = first["generation_id"].as_str().expect("generation ID");
    assert!(root
        .join("dist/distributed/generations")
        .join(generation)
        .join("generation.json")
        .is_file());
    assert!(root
        .join("dist/distributed/generations")
        .join(generation)
        .join("release.json")
        .is_file());

    fs::write(root.join("src/input.txt"), "first\n").expect("rewrite identical content");
    let second = distributed(&root, &[]);
    assert!(second.status.success());
    let second = report(&second);
    assert_eq!(first["generation_id"], second["generation_id"]);
    assert_eq!(first["release_id"], second["release_id"]);

    let snapshot_before_check = file_snapshot(&root);
    let check = distributed(&root, &["--check"]);
    assert!(
        check.status.success(),
        "{}",
        String::from_utf8_lossy(&check.stderr)
    );
    assert!(report(&check)["drift"].as_array().unwrap().is_empty());
    assert_eq!(
        active_before,
        fs::read(root.join("dist/distributed/active.json")).unwrap()
    );
    assert_eq!(snapshot_before_check, file_snapshot(&root));
    fs::remove_dir_all(root).expect("remove lifecycle fixture");
}

#[test]
fn mixed_source_and_unowned_outputs_fail_before_activation() {
    let mixed_root = temporary_root("mixed-source");
    write_fixture(&mixed_root);
    let config_path = mixed_root.join("distributed.lifecycle.json");
    let mut config: Value = serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
    config["source"]["javascript"] = Value::String(format!("sha256:{}", "b".repeat(64)));
    fs::write(&config_path, serde_json::to_vec_pretty(&config).unwrap()).unwrap();
    let mixed = distributed(&mixed_root, &[]);
    assert!(!mixed.status.success());
    let diagnostic = String::from_utf8_lossy(&mixed.stderr);
    assert!(diagnostic.contains(&format!("sha256:{}", "a".repeat(64))));
    assert!(diagnostic.contains(&format!("sha256:{}", "b".repeat(64))));
    assert!(!mixed_root.join("dist/distributed/active.json").exists());
    fs::remove_dir_all(mixed_root).unwrap();

    let unowned_root = temporary_root("unowned-output");
    write_fixture(&unowned_root);
    fs::write(unowned_root.join("produce-unowned"), "yes\n").unwrap();
    let unowned = distributed(&unowned_root, &[]);
    assert!(!unowned.status.success());
    assert!(
        String::from_utf8_lossy(&unowned.stderr).contains("unowned staged output `unowned.txt`")
    );
    assert!(!unowned_root.join("dist/distributed/active.json").exists());
    fs::remove_dir_all(unowned_root).unwrap();
}

#[test]
fn dev_waits_for_initial_generation_and_restarts_only_invalidated_processes() {
    let root = temporary_root("dev-supervisor");
    write_fixture(&root);
    enable_dev(&root);
    let stop = Arc::new(AtomicBool::new(false));
    let supervisor_stop = Arc::clone(&stop);
    let supervisor_root = root.clone();
    let supervisor = thread::spawn(move || {
        run_lifecycle_dev(&LifecycleDevOptions {
            build: LifecycleBuildOptions {
                root: supervisor_root,
                catalog: "distributed.contracts.json".into(),
                config: "distributed.lifecycle.json".into(),
                out: "dist/distributed".into(),
                check: false,
                lock_timeout: Duration::from_secs(1),
            },
            stop: supervisor_stop,
        })
    });

    wait_until(Duration::from_secs(5), || {
        fs::read_to_string(root.join("dev-process.log")).is_ok_and(|log| log.lines().count() == 2)
    });
    let initial_log = fs::read_to_string(root.join("dev-process.log")).unwrap();
    assert!(initial_log
        .lines()
        .all(|line| line.contains(":present:sha256:")));
    thread::sleep(Duration::from_millis(100));
    fs::write(root.join("src/input.txt"), "second\n").unwrap();
    wait_until(Duration::from_secs(5), || {
        fs::read_to_string(root.join("dev-process.log")).is_ok_and(|log| log.lines().count() == 3)
    });
    stop.store(true, Ordering::SeqCst);
    let report = supervisor
        .join()
        .expect("join lifecycle supervisor")
        .expect("lifecycle supervisor succeeds");
    assert_eq!(report.rebuilds, 1);
    assert_eq!(report.restarts["api"], 1);
    assert_eq!(report.restarts["ui"], 0);
    let log = fs::read_to_string(root.join("dev-process.log")).unwrap();
    assert_eq!(
        log.lines().filter(|line| line.starts_with("api:")).count(),
        2
    );
    assert_eq!(
        log.lines().filter(|line| line.starts_with("ui:")).count(),
        1
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn stale_check_names_owner_and_failed_downstream_build_preserves_active_generation() {
    let root = temporary_root("failure");
    write_fixture(&root);
    let baseline = distributed(&root, &[]);
    assert!(baseline.status.success());
    let active_before = fs::read(root.join("dist/distributed/active.json")).unwrap();

    fs::write(root.join("generated/plan.json"), "stale\n").expect("make tracked output stale");
    let check = distributed(&root, &["--check"]);
    assert_eq!(check.status.code(), Some(1));
    let check = report(&check);
    assert_eq!(check["drift"][0]["node_id"], "plan");
    assert_eq!(check["drift"][0]["output"], "generated/plan.json");
    assert!(check["drift"][0]["built_identity"]
        .as_str()
        .unwrap()
        .starts_with("sha256:"));
    assert!(check["drift"][0]["workspace_identity"]
        .as_str()
        .unwrap()
        .starts_with("sha256:"));

    fs::write(root.join("src/input.txt"), "second\n").expect("change lifecycle source");
    fs::write(root.join("fail-plan"), "fail\n").expect("inject downstream failure");
    let failed = distributed(&root, &[]);
    assert!(!failed.status.success());
    assert!(String::from_utf8_lossy(&failed.stderr).contains("lifecycle node `plan`"));
    assert_eq!(
        active_before,
        fs::read(root.join("dist/distributed/active.json")).unwrap()
    );
    fs::remove_dir_all(root).expect("remove lifecycle fixture");
}
