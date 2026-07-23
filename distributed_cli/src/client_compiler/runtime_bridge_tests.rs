use std::collections::BTreeMap;

use super::graphql::{compile_document, CompiledOperation};
use super::manifest::ClientManifest;
use super::render::{render_operation_artifact_json, render_operation_module};
use super::tests::manifest;
use super::{ClientDocument, ClientSurfaceSelector};

const RUNTIME_BRIDGE_QUERY: &str = r#"
  query RustRuntimeBridge($id: ID!, $tenantId: ID!) {
    todo(id: $id, tenantId: $tenantId) {
      id
      title
    }
  }
"#;

fn compile_runtime_bridge_operation() -> (ClientManifest, CompiledOperation) {
    let manifest = ClientManifest::parse(manifest(), &ClientSurfaceSelector::role("user"))
        .expect("runtime bridge manifest");
    let document = ClientDocument::new(
        "src/routes/runtime-bridge/+page.graphql",
        RUNTIME_BRIDGE_QUERY,
    );
    let operation = compile_document(&document, &manifest, &BTreeMap::new())
        .expect("compile runtime bridge operation");
    (manifest, operation)
}

fn rust_runtime_bridge_artifact() -> String {
    let (manifest, operation) = compile_runtime_bridge_operation();
    let artifact = render_operation_artifact_json(&operation, &manifest)
        .expect("serialize runtime bridge operation artifact");
    format!("{artifact}\n")
}

#[test]
fn rust_emitted_runtime_bridge_artifact_is_byte_exact() {
    assert_eq!(
        rust_runtime_bridge_artifact(),
        include_str!("../../tests/fixtures/runtime-bridge-operation.json"),
        "the JavaScript runtime bridge fixture must remain exact Rust compiler output"
    );
}

#[test]
fn scalar_only_operation_module_is_byte_exact_and_needs_no_replica_value_import() {
    let (manifest, operation) = compile_runtime_bridge_operation();
    let module = render_operation_module(&operation, &manifest)
        .expect("render scalar-only operation module");
    assert_eq!(
        module,
        include_str!("../../tests/fixtures/generated-scalar-operation.ts"),
        "the strict TypeScript scalar-only fixture must remain exact Rust compiler output"
    );
    assert!(
        module.contains(
            "import type { ReplicaOperationArtifact } from '@hops-ops/distributed/replica';"
        ),
        "{module}"
    );
    assert!(!module.contains("ReplicaValue"), "{module}");
}
