//! Deterministic application lifecycle graph and generation records.
//!
//! This module composes the existing contract catalog. It never owns artifact
//! payload semantics and never executes a catalog generator as a shell command.

mod graph;
mod receipt;

pub use graph::{
    DistributedSourceIdentity, LifecycleConfig, LifecycleError, LifecycleGraph, LifecycleNode,
    LIFECYCLE_CONFIG_SCHEMA_VERSION, LIFECYCLE_GRAPH_SCHEMA_VERSION, MAX_LIFECYCLE_NODES,
};
pub use receipt::{
    ArtifactNodeReceipt, GenerationManifest, GENERATION_MANIFEST_SCHEMA_VERSION,
    NODE_RECEIPT_SCHEMA_VERSION,
};

pub(crate) fn digest_bytes(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};

    let digest = Sha256::digest(bytes);
    let hex = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("sha256:{hex}")
}

pub(crate) fn validate_stable_value(value: &str, label: &str) -> Result<(), LifecycleError> {
    if value.is_empty() || value != value.trim() || value.len() > 4096 {
        return Err(LifecycleError::new(format!(
            "{label} must be a non-empty bounded stable value"
        )));
    }
    if value.chars().any(char::is_control) {
        return Err(LifecycleError::new(format!(
            "{label} contains control characters"
        )));
    }
    Ok(())
}

pub(crate) fn validate_portable_path(value: &str, label: &str) -> Result<(), LifecycleError> {
    use std::path::{Component, Path};

    if value.is_empty() || value.starts_with('/') || value.contains(['\\', '\0']) {
        return Err(LifecycleError::new(format!(
            "{label} `{value}` is not a portable relative path"
        )));
    }
    if Path::new(value).components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(LifecycleError::new(format!(
            "{label} `{value}` escapes the lifecycle root"
        )));
    }
    Ok(())
}
