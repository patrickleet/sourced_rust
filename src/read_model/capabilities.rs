//! Capability descriptors read-model adapters advertise for writes and loads.

/// Adapter capabilities used to validate a write plan before any storage write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadModelAdapterCapabilities {
    pub relational_rows: bool,
    pub sparse_patches: bool,
    pub deletes: bool,
}

impl Default for ReadModelAdapterCapabilities {
    fn default() -> Self {
        Self {
            relational_rows: true,
            sparse_patches: true,
            deletes: true,
        }
    }
}

/// Adapter capabilities for primary-key relational read-model loads.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReadModelQueryCapabilities {
    pub relationship_includes: bool,
}

impl ReadModelQueryCapabilities {
    pub fn relationship_includes() -> Self {
        Self {
            relationship_includes: true,
        }
    }
}
