//! Capability descriptors read-model adapters advertise for loads.

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
