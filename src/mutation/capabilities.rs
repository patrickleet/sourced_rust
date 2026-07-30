//! Typed internal mutation capabilities generated from `ReadModel` metadata.

use serde::Serialize;

use crate::projection::ProjectionValueType;

/// Stable identity of a read model for mutation targeting.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct MutationModelIdentity {
    /// Logical model name (struct name).
    pub model: String,
    /// Opaque storage / table identity.
    pub storage: String,
}

/// One typed key column generated for a read model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MutationKeyCapability {
    /// Field / column name.
    pub name: String,
    /// Portable value type.
    pub value_type: ProjectionValueType,
    /// Zero-based primary-key ordinal.
    pub ordinal: u32,
}

/// One typed writable field generated for a read model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MutationFieldCapability {
    /// Field / column name.
    pub name: String,
    /// Portable value type.
    pub value_type: ProjectionValueType,
    /// Whether the field is nullable.
    pub nullable: bool,
    /// Whether the field may be explicitly unset.
    pub supports_unset: bool,
}

/// One relationship target generated for a read model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MutationRelationshipCapability {
    /// Relationship field name.
    pub name: String,
    /// Source model name.
    pub source_model: String,
    /// Target model name.
    pub target_model: String,
}

/// Complete typed internal mutation capability surface for one read model.
///
/// These descriptors are compiler/runtime metadata. They never appear as
/// public GraphQL CRUD fields.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ReadModelMutationCapabilities {
    /// Model identity.
    pub identity: MutationModelIdentity,
    /// Ordered primary-key fields.
    pub key: Vec<MutationKeyCapability>,
    /// Ordered writable body fields (including key fields for complete writes).
    pub fields: Vec<MutationFieldCapability>,
    /// Declared relationships.
    pub relationships: Vec<MutationRelationshipCapability>,
    /// Default returning selection (all non-hidden stored columns).
    pub returning: Vec<String>,
}

impl ReadModelMutationCapabilities {
    /// Construct capabilities from model metadata.
    pub fn new(
        model: impl Into<String>,
        storage: impl Into<String>,
        key: Vec<MutationKeyCapability>,
        fields: Vec<MutationFieldCapability>,
        relationships: Vec<MutationRelationshipCapability>,
    ) -> Self {
        let returning = fields.iter().map(|field| field.name.clone()).collect();
        Self {
            identity: MutationModelIdentity {
                model: model.into(),
                storage: storage.into(),
            },
            key,
            fields,
            relationships,
            returning,
        }
    }

    /// Return whether a field name is part of the primary key.
    pub fn is_key_field(&self, name: &str) -> bool {
        self.key.iter().any(|field| field.name == name)
    }

    /// Lookup a writable field capability.
    pub fn field(&self, name: &str) -> Option<&MutationFieldCapability> {
        self.fields.iter().find(|field| field.name == name)
    }
}
