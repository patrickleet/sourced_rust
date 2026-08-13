use std::fmt;

/// Errors raised while compiling portable application contracts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApplicationError {
    InvalidIdentity {
        kind: &'static str,
        value: String,
        reason: &'static str,
    },
    Duplicate {
        kind: &'static str,
        identity: String,
    },
    Collision {
        kind: &'static str,
        identity: String,
        reason: String,
    },
    Missing {
        kind: &'static str,
        identity: String,
    },
    InvalidSpec(String),
    UnsupportedVersion {
        expected: u32,
        actual: u32,
    },
    NonCanonical(&'static str),
    Canonical(String),
}

pub type ApplicationResult<T> = Result<T, ApplicationError>;

impl fmt::Display for ApplicationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidIdentity {
                kind,
                value,
                reason,
            } => write!(formatter, "invalid {kind} identity `{value}`: {reason}"),
            Self::Duplicate { kind, identity } => {
                write!(formatter, "duplicate {kind} identity `{identity}`")
            }
            Self::Collision {
                kind,
                identity,
                reason,
            } => write!(
                formatter,
                "colliding {kind} identity `{identity}`: {reason}"
            ),
            Self::Missing { kind, identity } => {
                write!(formatter, "missing {kind} identity `{identity}`")
            }
            Self::InvalidSpec(reason) => {
                write!(formatter, "invalid application specification: {reason}")
            }
            Self::UnsupportedVersion { expected, actual } => write!(
                formatter,
                "unsupported application manifest schema version {actual}; expected {expected}"
            ),
            Self::NonCanonical(kind) => write!(formatter, "non-canonical {kind} bytes"),
            Self::Canonical(reason) => {
                write!(formatter, "canonical application artifact error: {reason}")
            }
        }
    }
}

impl std::error::Error for ApplicationError {}

impl From<serde_json::Error> for ApplicationError {
    fn from(error: serde_json::Error) -> Self {
        Self::Canonical(error.to_string())
    }
}
