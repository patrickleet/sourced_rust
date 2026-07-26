pub(super) const PARTITION_ENCODING_DOMAIN: &[u8] = b"distributed.projection.scope-partition.v1\0";
pub(super) const RECORD_KEY_ENCODING_DOMAIN: &[u8] =
    b"distributed.projection.scope-record-key.v1\0";
pub(super) const COMPILED_TOPOLOGY_DOMAIN: &[u8] = b"distributed.projection.compiled-topology.v1\0";
pub(super) const COMPILED_TOPOLOGY_VERSION: u32 = 1;
pub(super) const SCOPE_CODEC_VERSION: u32 = 1;
pub(super) const MAX_PARTITION_PATH_DEPTH: usize = 32;
pub(super) const MAX_PARTITION_PATH_SEGMENT_BYTES: usize = 255;
