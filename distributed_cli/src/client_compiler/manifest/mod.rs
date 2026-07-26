mod commands;
mod model_validation;
mod parse;
mod projectors;
mod roots;
mod types;
mod util;

pub(crate) use super::{is_graphql_name, ClientCompileError, ClientSurfaceSelector};
pub(crate) use commands::*;
pub(crate) use model_validation::*;
pub(crate) use projectors::*;
pub(crate) use roots::*;
pub(crate) use util::{validate_hash, validate_nonempty};

const MANIFEST_VERSION: u64 = 7;
const PROTOCOL_VERSION: u64 = 2;
const PROTOCOL_FINGERPRINT: &str =
    "sha256:7631d15b16e327ff08d728e97ac5f90f5150d00774938e720bbbc6830b77e0cf";

pub(crate) use types::*;
pub(crate) use util::{canonical_json_value, hash_bytes, validate_exact_operation_hash};

#[cfg(test)]
pub(crate) use parse::refresh_schema_fingerprint;
