use super::*;
use serde_json::Value;
use std::path::{Path, PathBuf};

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
