use super::*;
use serde_json::Value;
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::fs::symlink;
#[cfg(unix)]
use std::os::unix::net::UnixListener;

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("distributed_cli has a repository parent")
        .to_path_buf()
}

fn fixture(name: &str) -> &'static str {
    match name {
        "valid" => include_str!("../../tests/fixtures/contracts/catalog/catalog-valid.json"),
        "duplicate" => {
            include_str!("../../tests/fixtures/contracts/catalog/catalog-duplicate-scope.json")
        }
        "escaping" => {
            include_str!("../../tests/fixtures/contracts/catalog/catalog-escaping-path.json")
        }
        "cycle" => include_str!("../../tests/fixtures/contracts/catalog/catalog-chain-cycle.json"),
        "environment" => {
            include_str!("../../tests/fixtures/contracts/catalog/catalog-environment-value.json")
        }
        _ => panic!("unknown catalog fixture {name}"),
    }
}

fn path_catalog(source: &str) -> ContractCatalog {
    let input = serde_json::json!({
        "schema_version": 1,
        "entries": {
            "path-test": {
                "id": "path-test",
                "kind": "migration_inventory",
                "scope": { "id": "path/test" },
                "owner": "test/path",
                "identity": {
                    "kind": "migration_inventory",
                    "value": "sha256:path-test"
                },
                "provenance": {
                    "sources": [source],
                    "generator": "test.path"
                },
                "outputs": { "output": "inside.txt" }
            }
        }
    });
    ContractCatalog::from_json_str(&input.to_string()).expect("path catalog JSON")
}

fn glob_catalog(source: &str, glob_limit: usize) -> ContractCatalog {
    let input = serde_json::json!({
        "schema_version": 1,
        "entries": {
            "glob-test": {
                "id": "glob-test",
                "kind": "migration_inventory",
                "scope": { "id": "path/glob" },
                "owner": "test/glob",
                "identity": {
                    "kind": "migration_inventory",
                    "value": "sha256:glob-test"
                },
                "provenance": {
                    "sources": [source],
                    "generator": "test.glob",
                    "glob_limit": glob_limit
                },
                "outputs": { "output": "inside.txt" }
            }
        }
    });
    ContractCatalog::from_json_str(&input.to_string()).expect("glob catalog JSON")
}

fn create_sparse_file(path: &Path, length: usize) {
    let file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)
        .expect("sparse file");
    file.set_len(length as u64).expect("sparse file length");
}

struct TemporaryDirectory(PathBuf);

impl TemporaryDirectory {
    fn new(label: &str) -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock after Unix epoch")
            .as_nanos();
        let base = std::env::temp_dir();
        for attempt in 0..100 {
            let path = base.join(format!(
                "distributed-contracts-{label}-{}-{timestamp}-{attempt}",
                std::process::id()
            ));
            if fs::create_dir(&path).is_ok() {
                return Self(path);
            }
        }
        panic!("could not create temporary directory for {label}");
    }

    fn path(&self) -> &Path {
        &self.0
    }

    #[cfg(unix)]
    fn new_short(label: &str) -> Self {
        let base = std::env::temp_dir();
        for attempt in 0..100 {
            let path = base.join(format!("dct-{label}-{}-{attempt}", std::process::id()));
            if fs::create_dir(&path).is_ok() {
                return Self(path);
            }
        }
        panic!("could not create short temporary directory for {label}");
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn catalog_references_one_declarative_client_inventory() {
    let root = repository_root();
    let catalog = ContractCatalog::from_path(root.join("distributed.contracts.json"))
        .expect("repository catalog should resolve without writing");
    let inventory =
        ClientInventory::from_path(root.join("tests/e2e-ui/ui/distributed.clients.json"))
            .expect("client inventory should use the shared schema");

    assert_eq!(inventory.clients.len(), 3);
    assert!(inventory
        .clients
        .iter()
        .any(|client| client.surface == "e2e-ui-admin"));
    assert_eq!(
        catalog.canonical_bytes().expect("canonical catalog"),
        ContractCatalog::from_json_str(
            std::str::from_utf8(&catalog.canonical_bytes().expect("canonical catalog"))
                .expect("canonical catalog is UTF-8")
        )
        .expect("canonical catalog parses")
        .canonical_bytes()
        .expect("second canonical catalog")
    );
}

#[test]
fn catalog_from_path_accepts_repository_relative_catalog_name() {
    if std::env::var_os("DISTRIBUTED_CATALOG_RELATIVE_PATH_CHILD").is_some() {
        ContractCatalog::from_path("distributed.contracts.json")
            .expect("repository-relative catalog path should resolve from the repository root");
        return;
    }

    let status = Command::new(std::env::current_exe().expect("contract test executable"))
        .current_dir(repository_root())
        .env("DISTRIBUTED_CATALOG_RELATIVE_PATH_CHILD", "1")
        .args([
            "--exact",
            "contracts::tests::catalog_from_path_accepts_repository_relative_catalog_name",
            "--quiet",
        ])
        .status()
        .expect("run repository-relative catalog test from the repository root");
    assert!(status.success());
}

#[test]
fn catalog_and_inventory_reject_sparse_oversized_files_before_reading() {
    let root = TemporaryDirectory::new("sparse-input");
    let catalog_path = root.path().join("distributed.contracts.json");
    create_sparse_file(&catalog_path, MAX_CATALOG_BYTES + 1);
    let catalog_error = ContractCatalog::from_path(&catalog_path)
        .expect_err("sparse oversized catalog must fail before allocation");
    assert_eq!(
        catalog_error.code(),
        ContractDiagnosticCode::CatalogInputLimit
    );

    let inventory_path = root.path().join("distributed.clients.json");
    create_sparse_file(&inventory_path, MAX_CATALOG_BYTES + 1);
    let inventory_error = ClientInventory::from_path(&inventory_path)
        .expect_err("sparse oversized inventory must fail before allocation");
    assert_eq!(
        inventory_error.code(),
        ContractDiagnosticCode::CatalogInputLimit
    );
}

#[test]
fn catalog_rejects_duplicate_scope_and_escaping_paths() {
    let duplicate = ContractCatalog::from_json_str(fixture("duplicate"))
        .expect_err("duplicate scopes must fail");
    assert_eq!(
        duplicate.code(),
        ContractDiagnosticCode::CatalogDuplicateScope
    );

    let escaping = ContractCatalog::from_json_str(fixture("escaping"))
        .expect_err("parent traversal must fail");
    assert_eq!(escaping.code(), ContractDiagnosticCode::CatalogPath);
}

#[test]
fn catalog_rejects_unknown_kinds_duplicate_owners_outputs_and_unbounded_globs() {
    let mut unknown_kind: Value =
        serde_json::from_str(fixture("valid")).expect("valid fixture JSON");
    unknown_kind["entries"]["application-manifest"]["kind"] =
        Value::String("future_artifact".to_string());
    let unknown_kind = ContractCatalog::from_json_str(
        &serde_json::to_string(&unknown_kind).expect("unknown kind JSON"),
    )
    .expect_err("unknown kinds must fail");
    assert_eq!(
        unknown_kind.code(),
        ContractDiagnosticCode::CatalogUnknownKind
    );

    let mut duplicate_owner: Value =
        serde_json::from_str(fixture("valid")).expect("valid fixture JSON");
    duplicate_owner["entries"]["deployment-plan"]["owner"] =
        duplicate_owner["entries"]["application-manifest"]["owner"].clone();
    let duplicate_owner = ContractCatalog::from_json_str(
        &serde_json::to_string(&duplicate_owner).expect("duplicate owner JSON"),
    )
    .expect_err("duplicate owners must fail");
    assert_eq!(
        duplicate_owner.code(),
        ContractDiagnosticCode::CatalogDuplicateOwner
    );

    let mut duplicate_output: Value =
        serde_json::from_str(fixture("valid")).expect("valid fixture JSON");
    duplicate_output["entries"]["deployment-plan"]["outputs"]["deployment"] =
        duplicate_output["entries"]["application-manifest"]["outputs"]["application"].clone();
    let duplicate_output = ContractCatalog::from_json_str(
        &serde_json::to_string(&duplicate_output).expect("duplicate output JSON"),
    )
    .expect_err("duplicate outputs must fail");
    assert_eq!(
        duplicate_output.code(),
        ContractDiagnosticCode::CatalogDuplicateOutput
    );

    let mut unbounded_glob: Value =
        serde_json::from_str(fixture("valid")).expect("valid fixture JSON");
    unbounded_glob["entries"]["application-manifest"]["provenance"]["sources"][0] =
        Value::String("migrations/**/*.sql".to_string());
    unbounded_glob["entries"]["application-manifest"]["provenance"]
        .as_object_mut()
        .expect("provenance object")
        .remove("glob_limit");
    let unbounded_glob = ContractCatalog::from_json_str(
        &serde_json::to_string(&unbounded_glob).expect("unbounded glob JSON"),
    )
    .expect_err("unbounded globs must fail");
    assert_eq!(
        unbounded_glob.code(),
        ContractDiagnosticCode::CatalogUnboundedGlob
    );
}

#[test]
fn catalog_rejects_recursive_globs_even_with_a_match_limit() {
    let mut recursive_glob: Value =
        serde_json::from_str(fixture("valid")).expect("valid fixture JSON");
    recursive_glob["entries"]["application-manifest"]["provenance"]["sources"][0] =
        Value::String("migrations/**/*.sql".to_string());
    recursive_glob["entries"]["application-manifest"]["provenance"]["glob_limit"] = Value::from(1);
    let recursive_glob = ContractCatalog::from_json_str(
        &serde_json::to_string(&recursive_glob).expect("recursive glob JSON"),
    )
    .expect_err("recursive globs must fail despite a match limit");
    assert_eq!(
        recursive_glob.code(),
        ContractDiagnosticCode::CatalogUnboundedGlob
    );
}

#[test]
fn catalog_glob_discovery_bounds_candidate_directory_entries() {
    let root = TemporaryDirectory::new("glob-candidates");
    fs::write(root.path().join("inside.txt"), b"output").expect("output fixture");
    let candidates = root.path().join("candidates");
    fs::create_dir(&candidates).expect("candidate directory");
    for index in 0..=MAX_CATALOG_DIRECTORY_ENTRIES {
        fs::write(
            candidates.join(format!("candidate-{index:04}.txt")),
            b"candidate",
        )
        .expect("candidate fixture");
    }

    let error = glob_catalog("candidates/*.sql", 1)
        .validate_paths(root.path())
        .expect_err("glob candidate traversal must be bounded independently of matches");
    assert_eq!(error.code(), ContractDiagnosticCode::CatalogInputLimit);
}

#[test]
fn catalog_accepts_a_glob_within_a_bounded_candidate_directory() {
    let root = TemporaryDirectory::new("bounded-glob");
    fs::write(root.path().join("inside.txt"), b"output").expect("output fixture");
    let candidates = root.path().join("candidates");
    fs::create_dir(&candidates).expect("candidate directory");
    fs::write(candidates.join("selected.sql"), b"candidate").expect("candidate fixture");

    glob_catalog("candidates/*.sql", 1)
        .validate_paths(root.path())
        .expect("bounded glob candidate traversal");
}

#[test]
fn catalog_rejects_absolute_paths_and_exhausting_filesystem_traversal() {
    let absolute = ContractCatalog::from_json_str(
        &serde_json::json!({
            "schema_version": 1,
            "entries": {
                "absolute": {
                    "id": "absolute",
                    "kind": "migration_inventory",
                    "scope": { "id": "path/absolute" },
                    "owner": "test/absolute",
                    "identity": {
                        "kind": "migration_inventory",
                        "value": "sha256:absolute"
                    },
                    "provenance": {
                        "sources": ["/etc/passwd"],
                        "generator": "test.absolute"
                    },
                    "outputs": { "output": "inside.txt" }
                }
            }
        })
        .to_string(),
    )
    .expect_err("absolute paths must fail");
    assert_eq!(absolute.code(), ContractDiagnosticCode::CatalogPath);

    let root = TemporaryDirectory::new("traversal");
    fs::write(root.path().join("inside.txt"), b"output").expect("output fixture");
    let empty_tree = root.path().join("empty-tree");
    fs::create_dir(&empty_tree).expect("empty tree");
    for index in 0..=MAX_CATALOG_DIRECTORIES {
        fs::create_dir(empty_tree.join(format!("directory-{index:04}")))
            .expect("empty child directory");
    }
    let traversal = path_catalog("empty-tree")
        .validate_paths(root.path())
        .expect_err("empty directories must be bounded");
    assert_eq!(traversal.code(), ContractDiagnosticCode::CatalogInputLimit);

    let deep_tree = root.path().join("deep-tree");
    fs::create_dir(&deep_tree).expect("deep tree");
    let mut current = deep_tree;
    for depth in 0..=MAX_CATALOG_DIRECTORY_DEPTH {
        current = current.join(format!("level-{depth:02}"));
        fs::create_dir(&current).expect("deep child directory");
    }
    let depth_error = path_catalog("deep-tree")
        .validate_paths(root.path())
        .expect_err("directory depth must be bounded");
    assert_eq!(
        depth_error.code(),
        ContractDiagnosticCode::CatalogInputLimit
    );

    let many_entries = root.path().join("many-entries");
    fs::create_dir(&many_entries).expect("many-entry directory");
    for index in 0..=MAX_CATALOG_DIRECTORY_ENTRIES {
        fs::write(many_entries.join(format!("file-{index:04}")), b"entry")
            .expect("many-entry file");
    }
    let entry_error = path_catalog("many-entries")
        .validate_paths(root.path())
        .expect_err("directory entries must be bounded");
    assert_eq!(
        entry_error.code(),
        ContractDiagnosticCode::CatalogInputLimit
    );
}

#[test]
fn catalog_rejects_excessive_json_bytes_and_depth() {
    let oversized = format!(
        "{{\"schema_version\":1,\"entries\":{{}},\"padding\":\"{}\"}}",
        "x".repeat(MAX_CATALOG_BYTES)
    );
    let byte_error = ContractCatalog::from_json_str(&oversized)
        .expect_err("oversized JSON must fail before parsing");
    assert_eq!(byte_error.code(), ContractDiagnosticCode::CatalogInputLimit);

    let mut deeply_nested = String::from("{\"padding\":");
    for _ in 0..=MAX_CATALOG_JSON_DEPTH {
        deeply_nested.push('[');
    }
    deeply_nested.push_str("null");
    for _ in 0..=MAX_CATALOG_JSON_DEPTH {
        deeply_nested.push(']');
    }
    deeply_nested.push('}');
    let depth_error =
        ContractCatalog::from_json_str(&deeply_nested).expect_err("deeply nested JSON must fail");
    assert_eq!(
        depth_error.code(),
        ContractDiagnosticCode::CatalogInputLimit
    );
}

#[cfg(unix)]
#[test]
fn catalog_rejects_symlink_and_special_file_paths() {
    let root = TemporaryDirectory::new_short("filesystem");
    let outside = TemporaryDirectory::new("outside");
    fs::write(root.path().join("inside.txt"), b"output").expect("output fixture");
    let outside_file = outside.path().join("outside.txt");
    fs::write(&outside_file, b"outside").expect("outside fixture");

    symlink(&outside_file, root.path().join("escaped")).expect("symlink fixture");
    let symlink_error = path_catalog("escaped")
        .validate_paths(root.path())
        .expect_err("symlink escapes must fail");
    assert_eq!(
        symlink_error.code(),
        ContractDiagnosticCode::CatalogSymlinkEscape
    );

    let socket_path = root.path().join("special.sock");
    let _listener = UnixListener::bind(&socket_path).expect("socket fixture");
    let special_error = path_catalog("special.sock")
        .validate_paths(root.path())
        .expect_err("special files must fail");
    assert_eq!(
        special_error.code(),
        ContractDiagnosticCode::CatalogSpecialFile
    );
}

#[test]
fn artifact_chain_rejects_cycles_kind_mismatch_and_environment_values() {
    let cycle =
        ContractCatalog::from_json_str(fixture("cycle")).expect_err("predecessor cycles must fail");
    assert_eq!(cycle.code(), ContractDiagnosticCode::ChainCycle);

    let mut missing_predecessor: Value =
        serde_json::from_str(fixture("valid")).expect("valid fixture JSON");
    missing_predecessor["entries"]["deployment-plan"]["predecessor"]["entry_id"] =
        Value::String("missing-predecessor".to_string());
    let missing_predecessor = ContractCatalog::from_json_str(
        &serde_json::to_string(&missing_predecessor).expect("missing predecessor JSON"),
    )
    .expect_err("missing predecessor must fail");
    assert_eq!(
        missing_predecessor.code(),
        ContractDiagnosticCode::ChainMissingPredecessor
    );

    let mut kind_mismatch: Value =
        serde_json::from_str(fixture("valid")).expect("valid fixture JSON");
    kind_mismatch["entries"]["deployment-plan"]["predecessor"]["identity"]["kind"] =
        Value::String("deployment_plan".to_string());
    let kind_mismatch = ContractCatalog::from_json_str(
        &serde_json::to_string(&kind_mismatch).expect("kind mismatch JSON"),
    )
    .expect_err("predecessor kind mismatch must fail");
    assert_eq!(
        kind_mismatch.code(),
        ContractDiagnosticCode::ChainKindMismatch
    );

    let environment = ContractCatalog::from_json_str(fixture("environment"))
        .expect_err("raw environment values must fail");
    assert_eq!(environment.code(), ContractDiagnosticCode::EnvironmentValue);
}

#[test]
fn human_and_json_diagnostics_preserve_same_facts_and_redact_values() {
    let migration = ContractDiagnostic::new(
        ContractDiagnosticCode::MigrationInventory,
        Some(ContractArtifactKind::MigrationInventory),
        Some("repository/migrations"),
        "distributed::migrations",
        ["migrations/postgres/0005_new.sql"],
        ["distributed.contracts.json"],
        None::<&str>,
        Some("sha256:before"),
        Some("sha256:after"),
        Some("add migration 5"),
        Some(true),
        "distributed contracts check --base <revision>",
    )
    .with_detail("migration inventory is missing a registered file");
    let schema = ContractDiagnostic::new(
        ContractDiagnosticCode::SchemaDrift,
        Some(ContractArtifactKind::SurfaceClientManifest),
        Some("e2e-ui"),
        "query-layer::surface",
        ["tests/e2e-ui/ui/distributed.clients.json"],
        ["tests/e2e-ui/ui/src/lib/generated/user/manifest.json"],
        Some("models.Todo.owner.nullable"),
        Some("postgres://user:password@example.invalid/db"),
        Some("Bearer hidden-token"),
        Some("surface schema drift"),
        None,
        "distributed contracts accept --scope client:e2e-ui",
    );
    let mut result = ContractCheckResult::default();
    result.push(migration.clone());
    result.push(schema.clone());

    let human = result.human();
    let json: Value = serde_json::from_str(&result.to_json().expect("diagnostic JSON"))
        .expect("diagnostic JSON object");
    assert!(human.contains("CTL-MIG-INVENTORY"));
    assert!(human.contains("CTL-SCHEMA-DRIFT"));
    assert!(human.contains("[REDACTED]"));
    assert!(!human.contains("postgres://"));
    assert_eq!(
        json["diagnostics"]
            .as_array()
            .expect("diagnostic list")
            .len(),
        2
    );
    assert_eq!(json["diagnostics"][1]["code"], "CTL-SCHEMA-DRIFT");
    assert_eq!(json["diagnostics"][1]["expected"], "[REDACTED]");
    assert_eq!(json["diagnostics"][1]["observed"], "[REDACTED]");
    assert_eq!(
        result,
        serde_json::from_str(&result.to_json().expect("diagnostic JSON"))
            .expect("diagnostic JSON round-trip")
    );
    assert!(!result
        .to_json()
        .expect("diagnostic JSON")
        .contains("postgres://"));
    assert_eq!(
        result.canonical_bytes().expect("canonical result"),
        result.canonical_bytes().expect("second canonical result")
    );

    let decoded: ContractDiagnostic = serde_json::from_value(serde_json::json!({
        "code": "CTL-SCHEMA-DRIFT",
        "scope": "secret=scope",
        "owner": "postgres://user:password@example.invalid/db",
        "source_paths": ["token=source"],
        "derived_paths": ["secret=derived"],
        "semantic_path": "password=semantic",
        "expected": "secret=expected",
        "observed": "Bearer observed-token",
        "required_classification": "token=classification",
        "repair_command": "secret=repair",
        "detail": "password=detail"
    }))
    .expect("diagnostic JSON round-trip");
    assert!(!decoded.human().contains("postgres://"));
    assert!(!decoded.human().contains("password="));
    assert!(!decoded.human().contains("token="));

    let mut directly_mutated = migration;
    directly_mutated.scope = Some("secret=scope".to_string());
    directly_mutated.owner = "postgres://user:password@example.invalid/db".to_string();
    directly_mutated
        .source_paths
        .insert("token=source".to_string());
    directly_mutated
        .derived_paths
        .insert("secret=derived".to_string());
    directly_mutated.semantic_path = Some("password=semantic".to_string());
    directly_mutated.required_classification = Some("token=classification".to_string());
    directly_mutated.repair_command = "secret=repair".to_string();
    directly_mutated.detail = "password=detail".to_string();

    let direct_human = directly_mutated.human();
    let direct_json = serde_json::to_string(&directly_mutated).expect("direct diagnostic JSON");
    let direct_debug = format!("{directly_mutated:?}");
    assert!(!direct_human.contains("postgres://"));
    assert!(!direct_human.contains("password="));
    assert!(!direct_human.contains("token="));
    assert!(!direct_json.contains("postgres://"));
    assert!(!direct_json.contains("password="));
    assert!(!direct_json.contains("token="));
    assert!(!direct_debug.contains("postgres://"));
    assert!(!direct_debug.contains("password="));
    assert!(!direct_debug.contains("token="));
}

#[test]
fn client_inventory_canonicalizes_client_and_document_order() {
    let inventory = ClientInventory::from_path(
        repository_root().join("tests/e2e-ui/ui/distributed.clients.json"),
    )
    .expect("client inventory");
    let mut reversed = inventory.clone();
    reversed.clients.reverse();
    assert_eq!(
        inventory.canonical_bytes().expect("canonical inventory"),
        reversed
            .canonical_bytes()
            .expect("reversed canonical inventory")
    );
}

#[test]
fn canonical_bytes_reject_direct_public_secret_mutation() {
    let mut catalog = ContractCatalog::from_json_str(fixture("valid")).expect("valid catalog");
    catalog
        .entries
        .get_mut("application-manifest")
        .expect("application entry")
        .identity
        .value = "postgres://user:password@example.invalid/db".to_string();
    let catalog_error = catalog
        .canonical_bytes()
        .expect_err("catalog serialization must validate public mutations");
    assert_eq!(
        catalog_error.code(),
        ContractDiagnosticCode::EnvironmentValue
    );

    let mut inventory = ClientInventory::from_json_str(include_str!(
        "../../../tests/e2e-ui/ui/distributed.clients.json"
    ))
    .expect("valid client inventory");
    inventory.clients[0]
        .documents
        .insert("src/routes/token=secret.graphql".to_string());
    let inventory_error = inventory
        .canonical_bytes()
        .expect_err("client serialization must validate public mutations");
    assert_eq!(
        inventory_error.code(),
        ContractDiagnosticCode::EnvironmentValue
    );
}

#[test]
fn check_and_aggregate_renderers_do_not_leak_mutated_artifact_identity() {
    let mut catalog = ContractCatalog::from_json_str(fixture("valid")).expect("valid catalog");
    catalog
        .entries
        .get_mut("application-manifest")
        .expect("application entry")
        .identity
        .value = "postgres://user:password@example.invalid/db".to_string();

    let checked = catalog.check(repository_root());
    assert!(checked.artifacts.is_empty());
    let checked_json = checked.to_json().expect("checked JSON");
    let checked_canonical =
        String::from_utf8(checked.canonical_bytes().expect("checked canonical JSON"))
            .expect("checked JSON is UTF-8");
    let checked_debug = format!("{checked:?}");
    assert!(!checked_json.contains("postgres://"));
    assert!(!checked_canonical.contains("postgres://"));
    assert!(!checked_debug.contains("postgres://"));

    let mut directly_mutated = ContractCheckResult {
        catalog_identity: Some("token=identity".to_string()),
        ..Default::default()
    };
    directly_mutated.artifacts.insert(
        "secret=entry".to_string(),
        ArtifactIdentity::new(
            ContractArtifactKind::ApplicationManifest,
            "password=artifact",
        ),
    );
    let direct_json = directly_mutated.to_json().expect("aggregate JSON");
    let direct_canonical = String::from_utf8(
        directly_mutated
            .canonical_bytes()
            .expect("aggregate canonical JSON"),
    )
    .expect("aggregate JSON is UTF-8");
    let direct_debug = format!("{directly_mutated:?}");
    assert!(!direct_json.contains("token=identity"));
    assert!(!direct_json.contains("secret=entry"));
    assert!(!direct_json.contains("password=artifact"));
    assert!(!direct_canonical.contains("token=identity"));
    assert!(!direct_canonical.contains("secret=entry"));
    assert!(!direct_canonical.contains("password=artifact"));
    assert!(!direct_debug.contains("token=identity"));
    assert!(!direct_debug.contains("secret=entry"));
    assert!(!direct_debug.contains("password=artifact"));
}

#[test]
fn client_inventory_rust_schema_matches_shared_parity_vectors() {
    let vectors: Value = serde_json::from_str(include_str!(
        "../../tests/fixtures/contracts/client-inventory-parity.json"
    ))
    .expect("client inventory parity JSON");
    for vector in vectors["vectors"].as_array().expect("parity vectors") {
        let name = vector["name"].as_str().expect("parity vector name");
        let expected = vector["valid"].as_bool().expect("parity vector result");
        let input = serde_json::to_string(&vector["inventory"]).expect("parity inventory JSON");
        assert_eq!(
            ClientInventory::from_json_str(&input).is_ok(),
            expected,
            "Rust client inventory parity vector `{name}`"
        );
    }
}
