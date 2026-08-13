//! Canonical projection partition and record-key encoding.
//!
//! A projector topology owns one codec registry. Command obligations and
//! projector-side row keys both pass through this registry, so field/column
//! aliases and scalar representations cannot silently produce different
//! record identities.

mod catalog;
mod constants;
mod errors;
mod key;
mod partition;
mod scope;
mod topology;

#[cfg(test)]
mod tests;

pub(crate) use catalog::{canonical_projection_topology_bytes, digest_projection_binding};
pub(crate) use errors::ProjectionScopeCodecError;
pub(crate) use partition::ProjectionPartitionSpec;
pub(crate) use scope::ProjectionScopeCodec;
pub(crate) use topology::{compile_projection_topology, CompiledProjectionTopology};
