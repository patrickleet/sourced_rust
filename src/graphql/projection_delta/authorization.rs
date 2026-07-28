use super::{DeltaKeyField, ProjectionDeltaError, ProjectionDeltaPartition};
use crate::{ResolvedProjectionKey, ResolvedProjectionPartition};

/// An authorized normalized model identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthorizedModel {
    /// Role/application-safe wire model identifier.
    pub wire_model: String,
    /// Authorized replacement mask for complete rows.
    pub replacement_fields: Vec<String>,
}

/// An authorized logical-to-wire field mapping.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthorizedField {
    /// Role-safe wire field name.
    pub wire_field: String,
}

/// An authorized, encoded normalized record identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthorizedRecordKey {
    /// Role-safe wire model identifier.
    pub wire_model: String,
    /// Complete ordered client key.
    pub fields: Vec<DeltaKeyField>,
}

/// An explicit authorized relationship identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthorizedRelationship {
    /// Role-safe wire relationship identifier.
    pub wire_relationship: String,
    /// Role-safe source model identifier.
    pub source_wire_model: String,
    /// Role-safe target model identifier.
    pub target_wire_model: String,
}

/// Authorization boundary between logical projection provenance and client
/// physical/wire identities.
///
/// Implementations must resolve exclusively from an already selected
/// role/application Surface. Returning `None` means the identity is denied or
/// cannot be represented safely; lowerers then recover at the narrowest
/// authorized scope without serializing the denied logical name.
pub trait ProjectionAuthorization {
    /// Map one logical partition to unit or an opaque role-safe token.
    ///
    /// Raw partition values must never cross this boundary.
    fn partition(
        &self,
        logical_partition: &ResolvedProjectionPartition,
    ) -> Result<Option<ProjectionDeltaPartition>, ProjectionDeltaError>;

    /// Map one logical model to its authorized client identity and field mask.
    fn model(&self, logical_model: &str) -> Option<AuthorizedModel>;

    /// Map one visible logical field to its authorized client wire field.
    fn field(&self, logical_model: &str, logical_field: &str) -> Option<AuthorizedField>;

    /// Encode a complete logical key through the selected model codecs.
    ///
    /// # Errors
    ///
    /// Returns a typed error for a malformed authorized mapping. `Ok(None)`
    /// means the key is not authorized or not safely addressable.
    fn record_key(
        &self,
        logical_model: &str,
        logical_key: &ResolvedProjectionKey,
    ) -> Result<Option<AuthorizedRecordKey>, ProjectionDeltaError>;

    /// Resolve one explicit selected relationship. Operational join storage
    /// names must not be returned.
    fn relationship(
        &self,
        source_logical_model: &str,
        logical_relationship: &str,
        target_logical_model: &str,
    ) -> Option<AuthorizedRelationship>;
}
