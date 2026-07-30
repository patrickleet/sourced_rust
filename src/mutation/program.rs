//! Canonical event-independent mutation programs.

use std::fmt;

use serde::{Serialize, Serializer};

use crate::projection::{
    ProjectionField, ProjectionInvalidation, ProjectionKeyField, ProjectionMutationKind,
    ProjectionOperation, ProjectionRelationshipEffect, ProjectionTarget, ProjectionValueType,
    MAX_PROJECTION_EXPRESSION_DEPTH, MAX_PROJECTION_OPERATIONS_PER_OCCURRENCE,
    MAX_PROJECTION_PATH_SEGMENTS,
};
use crate::projection_protocol::{MAX_PROJECTION_PARTITION_BYTES, MAX_PROJECTION_RECORD_KEY_BYTES};

use super::canonical::{canonical_json_bytes, digest_program};
use super::expression::{non_empty, MutationAssignment, MutationExpression};
use super::MutationProgramError;

/// Canonical portable mutation IR version.
pub const MUTATION_PROGRAM_IR_VERSION: u16 = 1;

/// Canonical logical mutation semantics version.
pub const MUTATION_OPERATION_SEMANTICS_VERSION: u16 = 1;

/// Maximum operations one mutation program may contain.
pub const MAX_MUTATION_OPERATIONS: usize = MAX_PROJECTION_OPERATIONS_PER_OCCURRENCE;

/// Version-one resource ceilings included in every program digest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct MutationProgramLimits {
    expression_value_levels: u16,
    path_segments: u16,
    operations_per_program: u16,
    key_bytes: u32,
    partition_bytes: u32,
}

impl MutationProgramLimits {
    fn version_one() -> Self {
        Self {
            expression_value_levels: MAX_PROJECTION_EXPRESSION_DEPTH as u16,
            path_segments: MAX_PROJECTION_PATH_SEGMENTS as u16,
            operations_per_program: MAX_MUTATION_OPERATIONS as u16,
            key_bytes: MAX_PROJECTION_RECORD_KEY_BYTES as u32,
            partition_bytes: MAX_PROJECTION_PARTITION_BYTES as u32,
        }
    }

    /// Return the expression nesting ceiling.
    pub fn expression_value_levels(&self) -> u16 {
        self.expression_value_levels
    }

    /// Return the path segment ceiling.
    pub fn path_segments(&self) -> u16 {
        self.path_segments
    }

    /// Return the operation ceiling.
    pub fn operations_per_program(&self) -> u16 {
        self.operations_per_program
    }

    /// Return the portable logical key encoding ceiling.
    pub fn key_bytes(&self) -> u32 {
        self.key_bytes
    }

    /// Return the portable logical partition encoding ceiling.
    pub fn partition_bytes(&self) -> u32 {
        self.partition_bytes
    }
}

/// How a complete-row write resolves primary-key conflicts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MutationConflictTarget {
    /// Use the model's declared primary key.
    PrimaryKey,
}

/// One ordered component of a logical composite key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MutationKeyField {
    ordinal: u32,
    name: String,
    expression: MutationExpression,
}

impl MutationKeyField {
    /// Construct one explicitly ordered key component.
    ///
    /// # Errors
    ///
    /// Rejects an empty field name.
    pub fn try_new(
        ordinal: u32,
        name: impl Into<String>,
        expression: MutationExpression,
    ) -> Result<Self, MutationProgramError> {
        Ok(Self {
            ordinal,
            name: non_empty(name.into(), "mutation key field")?,
            expression,
        })
    }

    /// Return the explicit zero-based key ordinal.
    pub fn ordinal(&self) -> u32 {
        self.ordinal
    }

    /// Return the stable key field name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Return the value expression.
    pub fn expression(&self) -> &MutationExpression {
        &self.expression
    }
}

/// One ordered field assignment in a logical mutation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MutationField {
    ordinal: u32,
    name: String,
    assignment: MutationAssignment,
}

impl MutationField {
    /// Construct one explicitly ordered field assignment.
    ///
    /// # Errors
    ///
    /// Rejects an empty field name.
    pub fn try_new(
        ordinal: u32,
        name: impl Into<String>,
        assignment: MutationAssignment,
    ) -> Result<Self, MutationProgramError> {
        Ok(Self {
            ordinal,
            name: non_empty(name.into(), "mutation field")?,
            assignment,
        })
    }

    /// Return the explicit zero-based field ordinal.
    pub fn ordinal(&self) -> u32 {
        self.ordinal
    }

    /// Return the stable field name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Return the portable assignment.
    pub fn assignment(&self) -> &MutationAssignment {
        &self.assignment
    }
}

/// Typed returning selection for authoritative server execution.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MutationReturning {
    fields: Vec<String>,
}

impl MutationReturning {
    /// Construct a returning selection.
    ///
    /// # Errors
    ///
    /// Rejects empty, duplicate, or blank field names.
    pub fn try_new(fields: Vec<String>) -> Result<Self, MutationProgramError> {
        if fields.is_empty() {
            return Err(MutationProgramError::InvalidReturning {
                reason: "returning requires at least one field".to_owned(),
            });
        }
        let mut seen = std::collections::BTreeSet::new();
        for field in &fields {
            if field.is_empty() {
                return Err(MutationProgramError::EmptyName("returning field"));
            }
            if !seen.insert(field.clone()) {
                return Err(MutationProgramError::DuplicateName {
                    kind: "returning field",
                    name: field.clone(),
                });
            }
        }
        Ok(Self { fields })
    }

    /// Construct a complete-model returning selection from ordered field names.
    ///
    /// # Errors
    ///
    /// Rejects invalid field lists.
    pub fn all(fields: Vec<String>) -> Result<Self, MutationProgramError> {
        Self::try_new(fields)
    }

    /// Return selected field names.
    pub fn fields(&self) -> &[String] {
        &self.fields
    }
}

/// Closed authoritative logical mutation set (event-independent).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MutationKind {
    /// Create a row and conflict if it already exists.
    Insert,
    /// Replace or create a complete row.
    Upsert,
    /// Modify selected fields of an existing row.
    Patch,
    /// Create a partial row or patch an existing row.
    UpsertPatch,
    /// Remove a row by its complete logical key.
    Delete,
    /// Delete and then insert a complete row as one logical mutation.
    Recreate,
    /// Insert a relationship-owned row.
    InsertRelated,
    /// Replace or create a complete relationship-owned row.
    UpsertRelated,
    /// Cache-only scoped invalidation (no authoritative row write).
    Invalidate,
}

impl MutationKind {
    pub(crate) fn is_complete_write(self) -> bool {
        matches!(
            self,
            Self::Insert
                | Self::Upsert
                | Self::Recreate
                | Self::InsertRelated
                | Self::UpsertRelated
        )
    }

    pub(crate) fn is_patch(self) -> bool {
        matches!(self, Self::Patch | Self::UpsertPatch)
    }

    pub(crate) fn is_related(self) -> bool {
        matches!(self, Self::InsertRelated | Self::UpsertRelated)
    }

    pub(crate) fn to_projection_kind(self) -> Option<ProjectionMutationKind> {
        Some(match self {
            Self::Insert => ProjectionMutationKind::Insert,
            Self::Upsert => ProjectionMutationKind::Upsert,
            Self::Patch => ProjectionMutationKind::Patch,
            Self::UpsertPatch => ProjectionMutationKind::UpsertPatch,
            Self::Delete => ProjectionMutationKind::Delete,
            Self::Recreate => ProjectionMutationKind::Recreate,
            Self::InsertRelated => ProjectionMutationKind::InsertRelated,
            Self::UpsertRelated => ProjectionMutationKind::UpsertRelated,
            Self::Invalidate => return None,
        })
    }
}

/// One ordered logical operation template within a mutation program.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MutationOperation {
    operation_id: String,
    staging_ordinal: u32,
    kind: MutationKind,
    target: ProjectionTarget,
    key: Vec<MutationKeyField>,
    fields: Vec<MutationField>,
    conflict: Option<MutationConflictTarget>,
    relationship_effects: Vec<ProjectionRelationshipEffect>,
    invalidations: Vec<ProjectionInvalidation>,
    returning: Option<MutationReturning>,
}

impl MutationOperation {
    /// Construct and validate one adapter-neutral logical operation.
    ///
    /// # Errors
    ///
    /// Rejects invalid ordinals, missing keys or fields, invalid relationship
    /// provenance, and `unset` in complete-row operations.
    #[allow(clippy::too_many_arguments)]
    pub fn try_new(
        operation_id: impl Into<String>,
        staging_ordinal: u32,
        kind: MutationKind,
        target: ProjectionTarget,
        mut key: Vec<MutationKeyField>,
        mut fields: Vec<MutationField>,
        conflict: Option<MutationConflictTarget>,
        mut relationship_effects: Vec<ProjectionRelationshipEffect>,
        mut invalidations: Vec<ProjectionInvalidation>,
        returning: Option<MutationReturning>,
    ) -> Result<Self, MutationProgramError> {
        let operation_id = non_empty(operation_id.into(), "mutation operation ID")?;
        key.sort_by_key(MutationKeyField::ordinal);
        fields.sort_by_key(MutationField::ordinal);
        validate_named_ordinals(
            &key,
            "mutation key field",
            MutationKeyField::ordinal,
            MutationKeyField::name,
        )?;
        validate_named_ordinals(
            &fields,
            "mutation field",
            MutationField::ordinal,
            MutationField::name,
        )?;
        if kind != MutationKind::Invalidate && key.is_empty() {
            return Err(MutationProgramError::InvalidOperation {
                operation: operation_id,
                reason: "a complete logical key is required".to_owned(),
            });
        }
        if kind == MutationKind::Delete && !fields.is_empty() {
            return Err(MutationProgramError::InvalidOperation {
                operation: operation_id,
                reason: "delete cannot declare fields".to_owned(),
            });
        }
        if matches!(
            kind,
            MutationKind::Insert
                | MutationKind::Upsert
                | MutationKind::Patch
                | MutationKind::UpsertPatch
                | MutationKind::Recreate
                | MutationKind::InsertRelated
                | MutationKind::UpsertRelated
        ) && fields.is_empty()
        {
            return Err(MutationProgramError::InvalidOperation {
                operation: operation_id,
                reason: "a write operation requires at least one field".to_owned(),
            });
        }
        if kind.is_complete_write()
            && fields
                .iter()
                .any(|field| matches!(field.assignment, MutationAssignment::Unset))
        {
            return Err(MutationProgramError::InvalidOperation {
                operation: operation_id,
                reason: "complete-row writes cannot unset fields".to_owned(),
            });
        }
        if kind.is_complete_write()
            && fields
                .iter()
                .any(|field| matches!(field.assignment, MutationAssignment::Unknown))
        {
            return Err(MutationProgramError::InvalidOperation {
                operation: operation_id,
                reason: "complete-row writes cannot contain unknown fields".to_owned(),
            });
        }
        if matches!(
            kind,
            MutationKind::Insert | MutationKind::Upsert | MutationKind::Recreate
        ) && conflict.is_none()
            && matches!(kind, MutationKind::Upsert | MutationKind::Recreate)
        {
            // Upsert defaults to primary_key when omitted.
        }
        if kind == MutationKind::Upsert && conflict.is_none() {
            // Will be treated as primary_key during canonicalization.
        }
        if kind == MutationKind::Invalidate && !fields.is_empty() {
            return Err(MutationProgramError::InvalidOperation {
                operation: operation_id,
                reason: "invalidate cannot declare fields".to_owned(),
            });
        }
        if kind.is_related() && relationship_effects.is_empty() {
            return Err(MutationProgramError::InvalidOperation {
                operation: operation_id,
                reason: "related-row writes require relationship provenance".to_owned(),
            });
        }
        relationship_effects.sort_by_key(ProjectionRelationshipEffect::ordinal);
        validate_ordinals(
            &relationship_effects,
            "relationship effect",
            ProjectionRelationshipEffect::ordinal,
        )?;
        invalidations.sort();
        invalidations.dedup();
        let conflict = match (kind, conflict) {
            (MutationKind::Upsert | MutationKind::Recreate, None) => {
                Some(MutationConflictTarget::PrimaryKey)
            }
            (MutationKind::Insert, Some(_)) => {
                return Err(MutationProgramError::InvalidConflictTarget {
                    reason: "insert cannot declare a conflict target".to_owned(),
                });
            }
            (_, conflict) => conflict,
        };
        Ok(Self {
            operation_id,
            staging_ordinal,
            kind,
            target,
            key,
            fields,
            conflict,
            relationship_effects,
            invalidations,
            returning,
        })
    }

    /// Construct a concise state-upsert sugar operation that expands to an
    /// explicit primary-key upsert of all input fields under `input_root`.
    ///
    /// # Errors
    ///
    /// Rejects empty IDs or field lists.
    pub fn state_upsert(
        operation_id: impl Into<String>,
        staging_ordinal: u32,
        target: ProjectionTarget,
        key_fields: &[(u32, &str)],
        body_fields: &[(u32, &str, ProjectionValueType)],
        input_root: &[&str],
    ) -> Result<Self, MutationProgramError> {
        let mut key = Vec::with_capacity(key_fields.len());
        for (ordinal, name) in key_fields {
            let mut path = input_root
                .iter()
                .map(|segment| (*segment).to_owned())
                .collect::<Vec<_>>();
            path.push((*name).to_owned());
            key.push(MutationKeyField::try_new(
                *ordinal,
                *name,
                MutationExpression::input_path(ProjectionValueType::String, path)?,
            )?);
        }
        let mut fields = Vec::with_capacity(body_fields.len() + key_fields.len());
        let mut ordinal = 0_u32;
        for (key_ordinal, name) in key_fields {
            let _ = key_ordinal;
            let mut path = input_root
                .iter()
                .map(|segment| (*segment).to_owned())
                .collect::<Vec<_>>();
            path.push((*name).to_owned());
            fields.push(MutationField::try_new(
                ordinal,
                *name,
                MutationAssignment::set(MutationExpression::input_path(
                    ProjectionValueType::String,
                    path,
                )?),
            )?);
            ordinal += 1;
        }
        for (field_ordinal, name, value_type) in body_fields {
            let _ = field_ordinal;
            let mut path = input_root
                .iter()
                .map(|segment| (*segment).to_owned())
                .collect::<Vec<_>>();
            path.push((*name).to_owned());
            fields.push(MutationField::try_new(
                ordinal,
                *name,
                MutationAssignment::set(MutationExpression::input_path(value_type.clone(), path)?),
            )?);
            ordinal += 1;
        }
        Self::try_new(
            operation_id,
            staging_ordinal,
            MutationKind::Upsert,
            target,
            key,
            fields,
            Some(MutationConflictTarget::PrimaryKey),
            Vec::new(),
            Vec::new(),
            None,
        )
    }

    /// Return the stable operation identifier.
    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }

    /// Return the explicit source staging ordinal.
    pub fn staging_ordinal(&self) -> u32 {
        self.staging_ordinal
    }

    /// Return the authoritative logical mutation kind.
    pub fn kind(&self) -> MutationKind {
        self.kind
    }

    /// Return the target read model.
    pub fn target(&self) -> &ProjectionTarget {
        &self.target
    }

    /// Return composite key fields in ordinal order.
    pub fn key(&self) -> &[MutationKeyField] {
        &self.key
    }

    /// Return projected fields in ordinal order.
    pub fn fields(&self) -> &[MutationField] {
        &self.fields
    }

    /// Return the conflict target when applicable.
    pub fn conflict(&self) -> Option<MutationConflictTarget> {
        self.conflict
    }

    /// Return ordered relationship consequences and provenance.
    pub fn relationship_effects(&self) -> &[ProjectionRelationshipEffect] {
        &self.relationship_effects
    }

    /// Return sorted invalidation inventory associated with this mutation.
    pub fn invalidations(&self) -> &[ProjectionInvalidation] {
        &self.invalidations
    }

    /// Return optional typed returning selection.
    pub fn returning(&self) -> Option<&MutationReturning> {
        self.returning.as_ref()
    }

    /// Rewrite this mutation operation into a projection operation using the
    /// supplied input binder.
    ///
    /// # Errors
    ///
    /// Rejects invalidate-only operations and rewrite failures.
    pub fn rewrite_to_projection(
        &self,
        bind_input_path: &dyn Fn(
            &[String],
            &ProjectionValueType,
        ) -> Result<
            crate::projection::ProjectionExpression,
            MutationProgramError,
        >,
    ) -> Result<ProjectionOperation, MutationProgramError> {
        let kind = self.kind.to_projection_kind().ok_or_else(|| {
            MutationProgramError::InvalidOperation {
                operation: self.operation_id.clone(),
                reason: "invalidate is cache-only and has no projection row operation".to_owned(),
            }
        })?;
        let key = self
            .key
            .iter()
            .map(|field| {
                ProjectionKeyField::try_new(
                    field.ordinal,
                    field.name.clone(),
                    field.expression.rewrite_with(bind_input_path)?,
                )
                .map_err(MutationProgramError::from)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut fields = Vec::new();
        for field in &self.fields {
            if let Some(assignment) = field.assignment.rewrite_with(bind_input_path)? {
                fields.push(
                    ProjectionField::try_new(field.ordinal, field.name.clone(), assignment)
                        .map_err(MutationProgramError::from)?,
                );
            }
        }
        // Re-number fields after unknown filtering so ordinals remain contiguous.
        fields.sort_by_key(ProjectionField::ordinal);
        let fields = fields
            .into_iter()
            .enumerate()
            .map(|(index, field)| {
                ProjectionField::try_new(index as u32, field.name(), field.assignment().clone())
                    .map_err(MutationProgramError::from)
            })
            .collect::<Result<Vec<_>, _>>()?;
        ProjectionOperation::try_new(
            self.operation_id.clone(),
            self.staging_ordinal,
            kind,
            self.target.clone(),
            key,
            fields,
            self.relationship_effects.clone(),
            self.invalidations.clone(),
        )
        .map_err(Into::into)
    }
}

/// A validated, canonical and adapter-neutral mutation program.
///
/// Mutation programs are event-independent. They contain no event selector,
/// event type, upcaster, owner, placement, or command-preview data.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MutationProgram {
    ir_version: u16,
    operation_semantics_version: u16,
    limits: MutationProgramLimits,
    name: String,
    version: u64,
    operations: Vec<MutationOperation>,
}

impl MutationProgram {
    /// Construct a canonical version-one mutation program.
    ///
    /// # Errors
    ///
    /// Rejects empty names, zero versions, empty programs, operation limits,
    /// duplicate operation IDs/ordinals, and ambiguous same-scope mutations.
    pub fn try_new(
        name: impl Into<String>,
        version: u64,
        mut operations: Vec<MutationOperation>,
    ) -> Result<Self, MutationProgramError> {
        let name = non_empty(name.into(), "mutation program name")?;
        if version == 0 {
            return Err(MutationProgramError::ZeroVersion(
                "mutation program version",
            ));
        }
        if operations.is_empty() {
            return Err(MutationProgramError::InvalidOperation {
                operation: name,
                reason: "a program requires at least one operation".to_owned(),
            });
        }
        if operations.len() > MAX_MUTATION_OPERATIONS {
            return Err(MutationProgramError::TooManyOperations {
                count: operations.len(),
                max: MAX_MUTATION_OPERATIONS,
            });
        }
        operations.sort_by_key(MutationOperation::staging_ordinal);
        validate_named_ordinals(
            &operations,
            "mutation operation",
            MutationOperation::staging_ordinal,
            MutationOperation::operation_id,
        )?;
        validate_static_ambiguity(&operations)?;
        Ok(Self {
            ir_version: MUTATION_PROGRAM_IR_VERSION,
            operation_semantics_version: MUTATION_OPERATION_SEMANTICS_VERSION,
            limits: MutationProgramLimits::version_one(),
            name,
            version,
            operations,
        })
    }

    /// Return the stable program name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Return the canonical portable IR version.
    pub fn ir_version(&self) -> u16 {
        self.ir_version
    }

    /// Return the closed logical mutation semantics version.
    pub fn operation_semantics_version(&self) -> u16 {
        self.operation_semantics_version
    }

    /// Return the resource ceilings bound into this program's identity.
    pub fn limits(&self) -> MutationProgramLimits {
        self.limits
    }

    /// Return the independently evolving program version.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// Return operations in explicit staging order.
    pub fn operations(&self) -> &[MutationOperation] {
        &self.operations
    }

    /// Encode canonical versioned JSON for manifests and golden vectors.
    ///
    /// # Errors
    ///
    /// Returns a typed error if canonical serialization fails.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, MutationProgramError> {
        canonical_json_bytes(self)
    }

    /// Compute the domain-separated identity of the complete program semantics.
    ///
    /// # Errors
    ///
    /// Returns a typed error if canonical serialization fails.
    pub fn id(&self) -> Result<MutationProgramId, MutationProgramError> {
        Ok(MutationProgramId(digest_program(&self.canonical_bytes()?)))
    }

    /// Rewrite every operation into projection operations.
    ///
    /// # Errors
    ///
    /// Propagates rewrite failures.
    pub fn rewrite_to_projection_operations(
        &self,
        bind_input_path: &dyn Fn(
            &[String],
            &ProjectionValueType,
        ) -> Result<
            crate::projection::ProjectionExpression,
            MutationProgramError,
        >,
    ) -> Result<Vec<ProjectionOperation>, MutationProgramError> {
        self.operations
            .iter()
            .filter(|operation| operation.kind != MutationKind::Invalidate)
            .map(|operation| operation.rewrite_to_projection(bind_input_path))
            .collect()
    }
}

/// Domain-separated digest of a complete canonical mutation program.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MutationProgramId([u8; 32]);

impl MutationProgramId {
    /// Parse a canonical `mp1:sha256:<lowercase-hex>` program identity.
    ///
    /// # Errors
    ///
    /// Rejects alternate prefixes, lengths, uppercase, and non-hex text.
    pub fn parse(value: &str) -> Result<Self, MutationProgramError> {
        let Some(hex) = value.strip_prefix("mp1:sha256:") else {
            return Err(MutationProgramError::InvalidProgramId);
        };
        if hex.len() != 64
            || !hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(MutationProgramError::InvalidProgramId);
        }
        let mut bytes = [0_u8; 32];
        for (index, pair) in hex.as_bytes().chunks_exact(2).enumerate() {
            let high = hex_nibble(pair[0]).ok_or(MutationProgramError::InvalidProgramId)?;
            let low = hex_nibble(pair[1]).ok_or(MutationProgramError::InvalidProgramId)?;
            bytes[index] = (high << 4) | low;
        }
        Ok(Self(bytes))
    }

    /// Return the raw SHA-256 digest.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for MutationProgramId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("mp1:sha256:")?;
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl Serialize for MutationProgramId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

/// Typed authoring handle for a validated mutation program with input type marker.
#[derive(Clone, Debug)]
pub struct Mutation<I> {
    program: MutationProgram,
    marker: std::marker::PhantomData<fn() -> I>,
}

impl<I> Mutation<I> {
    /// Construct a typed mutation handle from a validated program.
    pub fn from_program(program: MutationProgram) -> Self {
        Self {
            program,
            marker: std::marker::PhantomData,
        }
    }

    /// Return the canonical program.
    pub fn program(&self) -> &MutationProgram {
        &self.program
    }

    /// Return the program identity.
    ///
    /// # Errors
    ///
    /// Returns canonical encoding failures.
    pub fn id(&self) -> Result<MutationProgramId, MutationProgramError> {
        self.program.id()
    }
}

fn validate_static_ambiguity(operations: &[MutationOperation]) -> Result<(), MutationProgramError> {
    for (index, left) in operations.iter().enumerate() {
        for right in &operations[index + 1..] {
            if left.target != right.target {
                continue;
            }
            if left.key == right.key {
                let compatible = left.kind == right.kind
                    && left.relationship_effects == right.relationship_effects
                    && if left.kind.is_patch() {
                        left.fields.iter().all(|left_field| {
                            right
                                .fields
                                .iter()
                                .find(|right_field| right_field.name == left_field.name)
                                .is_none_or(|right_field| {
                                    right_field.assignment == left_field.assignment
                                })
                        })
                    } else {
                        left.fields == right.fields
                    };
                if !compatible {
                    return Err(MutationProgramError::AmbiguousMutation {
                        model: left.target.model().to_owned(),
                        reason: format!(
                            "operations `{}` and `{}` statically target the same key \
                             with order-dependent semantics",
                            left.operation_id, right.operation_id
                        ),
                    });
                }
            }
        }
    }
    Ok(())
}

fn validate_named_ordinals<T, FOrd, FName>(
    items: &[T],
    kind: &'static str,
    ordinal: FOrd,
    name: FName,
) -> Result<(), MutationProgramError>
where
    FOrd: Fn(&T) -> u32,
    FName: Fn(&T) -> &str,
{
    validate_ordinals(items, kind, &ordinal)?;
    let mut seen = std::collections::BTreeSet::new();
    for item in items {
        let item_name = name(item).to_owned();
        if !seen.insert(item_name.clone()) {
            return Err(MutationProgramError::DuplicateName {
                kind,
                name: item_name,
            });
        }
    }
    Ok(())
}

fn validate_ordinals<T, FOrd>(
    items: &[T],
    kind: &'static str,
    ordinal: FOrd,
) -> Result<(), MutationProgramError>
where
    FOrd: Fn(&T) -> u32,
{
    for (expected, item) in items.iter().enumerate() {
        let actual = ordinal(item);
        if actual != expected as u32 {
            return Err(MutationProgramError::NonContiguousOrdinal {
                kind,
                expected: expected as u32,
                actual,
            });
        }
    }
    // Detect duplicates that somehow pass contiguity (empty is fine).
    let mut previous = None;
    for item in items {
        let actual = ordinal(item);
        if previous == Some(actual) {
            return Err(MutationProgramError::DuplicateOrdinal {
                kind,
                ordinal: actual,
            });
        }
        previous = Some(actual);
    }
    Ok(())
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}
