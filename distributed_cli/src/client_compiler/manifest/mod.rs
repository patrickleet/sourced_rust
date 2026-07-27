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

const MANIFEST_VERSION: u64 = 1;
const PROTOCOL_VERSION: u64 = 1;
const PROTOCOL_FINGERPRINT: &str =
    "sha256:30f19c9f4d29280a02ddf67c4df62cdc92c4e8090792f43d6b1bdafea3e31273";

pub(crate) use types::*;
pub(crate) use util::{canonical_json_value, hash_bytes, validate_exact_operation_hash};

#[cfg(test)]
pub(crate) use parse::refresh_schema_fingerprint;
