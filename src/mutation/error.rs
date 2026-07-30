//! Typed validation and binding errors for mutation programs.

use std::fmt;

/// A typed validation, binding, or interpretation failure for a mutation program.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum MutationProgramError {
    /// A stable name, identifier, field, or storage name was empty.
    EmptyName(&'static str),
    /// A declared version must be non-zero.
    ZeroVersion(&'static str),
    /// A mutation program ID was not in canonical form.
    InvalidProgramId,
    /// An input path exceeded the portable segment limit.
    PathTooDeep {
        /// Observed segment count.
        segments: usize,
        /// Supported segment ceiling.
        max: usize,
    },
    /// A portable expression or constant exceeded the nesting limit.
    ExpressionTooDeep {
        /// Observed nesting depth.
        depth: usize,
        /// Supported nesting ceiling.
        max: usize,
    },
    /// A program declared too many operations.
    TooManyOperations {
        /// Observed operation count.
        count: usize,
        /// Supported operation ceiling.
        max: usize,
    },
    /// A key, partition, or other bounded canonical value was too large.
    ValueTooLarge {
        /// Kind of bounded logical encoding.
        kind: &'static str,
        /// Observed encoded byte length.
        len: usize,
        /// Supported byte ceiling.
        max: usize,
    },
    /// Two declarations used the same explicit ordinal.
    DuplicateOrdinal {
        /// Kind of ordered declaration.
        kind: &'static str,
        /// Repeated ordinal.
        ordinal: u32,
    },
    /// Ordinals did not form a contiguous zero-based sequence.
    NonContiguousOrdinal {
        /// Kind of ordered declaration.
        kind: &'static str,
        /// Required next ordinal.
        expected: u32,
        /// Observed ordinal.
        actual: u32,
    },
    /// Two declarations used the same stable name.
    DuplicateName {
        /// Kind of named declaration.
        kind: &'static str,
        /// Repeated name.
        name: String,
    },
    /// A required expression evaluated as absent.
    RequiredValueAbsent {
        /// Stable expression or field description.
        path: String,
    },
    /// `unset` appeared where the operation cannot represent it.
    UnsetNotAllowed {
        /// Field or expression position.
        field: String,
    },
    /// A key component was null, absent, unset, or structurally invalid.
    InvalidKeyValue {
        /// Key component name.
        field: String,
    },
    /// An operation shape was incompatible with its semantic kind.
    InvalidOperation {
        /// Stable operation identifier.
        operation: String,
        /// Validation reason.
        reason: String,
    },
    /// Two operations could not deterministically become one final mutation.
    AmbiguousMutation {
        /// Affected logical model.
        model: String,
        /// Validation reason.
        reason: String,
    },
    /// Returning selection was empty or ambiguous.
    InvalidReturning {
        /// Validation reason.
        reason: String,
    },
    /// Conflict target was incomplete or unknown.
    InvalidConflictTarget {
        /// Validation reason.
        reason: String,
    },
    /// Canonical JSON encoding failed.
    CanonicalJson(String),
    /// A bound input path was missing from the supplied input object.
    MissingInput {
        /// Missing input path.
        path: String,
    },
    /// A binding or interpreter failed while adapting to projection IR.
    Adapter(String),
}

impl fmt::Display for MutationProgramError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyName(kind) => write!(f, "{kind} must not be empty"),
            Self::ZeroVersion(kind) => write!(f, "{kind} must be non-zero"),
            Self::InvalidProgramId => write!(
                f,
                "mutation program ID must be `mp1:sha256:` followed by 64 lowercase hex digits"
            ),
            Self::PathTooDeep { segments, max } => {
                write!(f, "input path has {segments} segments; max is {max}")
            }
            Self::ExpressionTooDeep { depth, max } => {
                write!(f, "expression nesting depth {depth} exceeds max {max}")
            }
            Self::TooManyOperations { count, max } => {
                write!(f, "mutation program has {count} operations; max is {max}")
            }
            Self::ValueTooLarge { kind, len, max } => {
                write!(f, "{kind} encoding is {len} bytes; max is {max}")
            }
            Self::DuplicateOrdinal { kind, ordinal } => {
                write!(f, "duplicate {kind} ordinal {ordinal}")
            }
            Self::NonContiguousOrdinal {
                kind,
                expected,
                actual,
            } => write!(f, "{kind} ordinal expected {expected}, got {actual}"),
            Self::DuplicateName { kind, name } => write!(f, "duplicate {kind} name `{name}`"),
            Self::RequiredValueAbsent { path } => {
                write!(f, "required value absent at `{path}`")
            }
            Self::UnsetNotAllowed { field } => {
                write!(f, "unset is not allowed on field `{field}`")
            }
            Self::InvalidKeyValue { field } => write!(f, "invalid key value for `{field}`"),
            Self::InvalidOperation { operation, reason } => {
                write!(f, "invalid operation `{operation}`: {reason}")
            }
            Self::AmbiguousMutation { model, reason } => {
                write!(f, "ambiguous mutation for model `{model}`: {reason}")
            }
            Self::InvalidReturning { reason } => write!(f, "invalid returning: {reason}"),
            Self::InvalidConflictTarget { reason } => {
                write!(f, "invalid conflict target: {reason}")
            }
            Self::CanonicalJson(error) => write!(f, "canonical JSON encoding failed: {error}"),
            Self::MissingInput { path } => write!(f, "missing mutation input `{path}`"),
            Self::Adapter(error) => write!(f, "mutation adapter error: {error}"),
        }
    }
}

impl std::error::Error for MutationProgramError {}

impl From<crate::projection::ProjectionProgramError> for MutationProgramError {
    fn from(error: crate::projection::ProjectionProgramError) -> Self {
        Self::Adapter(error.to_string())
    }
}
