use std::collections::HashMap;
use std::time::SystemTime;

use crate::entity::{BITCODE_PAYLOAD_CODEC, BITCODE_PAYLOAD_CODEC_VERSION};
use crate::repository::{RepositoryError, StreamIdentity};

/// Stored aggregate snapshot cache record.
///
/// This is a repository-owned cache envelope, not an aggregate event and not
/// the user-defined state snapshot payload itself. The payload bytes usually
/// come from `Snapshottable::create_snapshot()`.
#[derive(Clone, Debug, PartialEq)]
pub struct SnapshotRecord {
    pub aggregate_type: String,
    pub aggregate_id: String,
    /// Aggregate event sequence covered by this cache record.
    pub version: u64,
    /// Diagnostic label for the payload type, typically
    /// `std::any::type_name::<A::Snapshot>()`. **Not** used to gate loads:
    /// `type_name` is explicitly unstable across compiler versions, so
    /// comparing it would cause spurious cache misses after a toolchain bump.
    /// Schema compatibility is enforced by [`snapshot_version`](Self::snapshot_version)
    /// against [`Snapshottable::SNAPSHOT_VERSION`](crate::Snapshottable::SNAPSHOT_VERSION).
    /// Retained for human-readable diagnostics only.
    pub snapshot_type: String,
    /// Snapshot payload schema version, written from
    /// [`Snapshottable::SNAPSHOT_VERSION`](crate::Snapshottable::SNAPSHOT_VERSION)
    /// at save time and compared on load. A mismatch is treated as a cache miss
    /// (full replay) rather than decoding a possibly-incompatible payload.
    pub snapshot_version: u64,
    pub payload_codec: String,
    pub payload_codec_version: u16,
    pub payload: Vec<u8>,
    pub metadata: HashMap<String, String>,
    pub recorded_at: SystemTime,
}

impl SnapshotRecord {
    /// Default snapshot schema version, mirroring
    /// [`Snapshottable::SNAPSHOT_VERSION`](crate::Snapshottable::SNAPSHOT_VERSION)'s
    /// default. The repository now writes `A::SNAPSHOT_VERSION` into each record
    /// and gates loads on it; this constant remains as the documented baseline
    /// for callers constructing records directly.
    pub const DEFAULT_SNAPSHOT_VERSION: u64 = 1;

    pub fn new(
        aggregate_type: impl Into<String>,
        aggregate_id: impl Into<String>,
        version: u64,
        snapshot_type: impl Into<String>,
        snapshot_version: u64,
        payload: Vec<u8>,
    ) -> Self {
        Self {
            aggregate_type: aggregate_type.into(),
            aggregate_id: aggregate_id.into(),
            version,
            snapshot_type: snapshot_type.into(),
            snapshot_version,
            payload_codec: BITCODE_PAYLOAD_CODEC.to_string(),
            payload_codec_version: BITCODE_PAYLOAD_CODEC_VERSION,
            payload,
            metadata: HashMap::new(),
            recorded_at: SystemTime::now(),
        }
    }

    pub fn validate_for_identity(&self, identity: &StreamIdentity) -> Result<(), RepositoryError> {
        self.validate()?;
        if self.aggregate_type != identity.aggregate_type() {
            return Err(RepositoryError::Model(format!(
                "snapshot aggregate type `{}` does not match stream identity `{}`",
                self.aggregate_type, identity
            )));
        }
        if self.aggregate_id != identity.aggregate_id() {
            return Err(RepositoryError::Model(format!(
                "snapshot aggregate id `{}` does not match stream identity `{}`",
                self.aggregate_id, identity
            )));
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<(), RepositoryError> {
        if self.aggregate_type.trim().is_empty() {
            return Err(RepositoryError::Model(
                "snapshot aggregate type must not be empty".into(),
            ));
        }
        if self.aggregate_id.trim().is_empty() {
            return Err(RepositoryError::Model(
                "snapshot aggregate id must not be empty".into(),
            ));
        }
        if self.version == 0 {
            return Err(RepositoryError::Model(
                "snapshot version must be greater than zero".into(),
            ));
        }
        if self.snapshot_type.trim().is_empty() {
            return Err(RepositoryError::Model(
                "snapshot type must not be empty".into(),
            ));
        }
        if self.snapshot_version == 0 {
            return Err(RepositoryError::Model(
                "snapshot payload version must be greater than zero".into(),
            ));
        }
        if self.payload_codec.trim().is_empty() {
            return Err(RepositoryError::Model(
                "snapshot payload codec must not be empty".into(),
            ));
        }
        if self.payload_codec_version == 0 {
            return Err(RepositoryError::Model(
                "snapshot payload codec version must be greater than zero".into(),
            ));
        }
        Ok(())
    }

    pub fn has_supported_payload_codec(&self) -> bool {
        self.payload_codec == BITCODE_PAYLOAD_CODEC
            && self.payload_codec_version == BITCODE_PAYLOAD_CODEC_VERSION
    }
}
