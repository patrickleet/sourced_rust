mod commands;
mod model_validation;
mod parse;
mod projections;
mod projectors;
mod roots;
mod types;
mod util;

pub(crate) use super::{is_graphql_name, ClientCompileError, ClientSurfaceSelector};
pub(crate) use commands::*;
pub(crate) use model_validation::*;
pub(crate) use projections::*;
pub(crate) use projectors::*;
pub(crate) use roots::*;
pub(crate) use util::{validate_hash, validate_nonempty};

const MANIFEST_VERSION: u64 = 2;
const PROTOCOL_VERSION: u64 = 1;
const PROTOCOL_FINGERPRINT: &str =
    "sha256:00fb342f3acb4dc1c1716a43cc3001c748d5f6c500ff831690d820e9e43e2782";

pub(crate) const CLIENT_PROJECTION_PROGRAM_VERSION: u32 = 2;
pub(crate) const CLIENT_PROJECTION_BINDING_VERSION: u32 = 1;
pub(crate) const COMMAND_PROJECTION_EXTENSION_VERSION: u32 = 2;
pub(crate) const PROJECTION_PROGRAM_IR_VERSION: u16 = 1;
pub(crate) const PROJECTION_OPERATION_SEMANTICS_VERSION: u16 = 1;

pub(crate) use types::*;
pub(crate) use util::{canonical_json_value, hash_bytes, validate_exact_operation_hash};

#[cfg(test)]
pub(crate) use parse::refresh_schema_fingerprint;
