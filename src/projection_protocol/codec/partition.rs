use super::constants::{MAX_PARTITION_PATH_DEPTH, MAX_PARTITION_PATH_SEGMENT_BYTES};
use crate::projection_protocol::{ProjectionProtocolError, MAX_PROJECTION_PARTITION_BYTES};

/// Canonical declaration-owned partition derivation for one projector.
///
/// The runtime evaluates this closed IR from raw JSON before decoding the
/// typed event. It is also part of the compiled topology digest, so changing
/// partition semantics necessarily creates a different durable topology.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum ProjectionPartitionSpec {
    Unit,
    InputPath { path: Vec<String> },
    Constant { value: serde_json::Value },
}

impl ProjectionPartitionSpec {
    pub(crate) fn unit() -> Self {
        Self::Unit
    }

    pub(crate) fn input_path(path: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self::InputPath {
            path: path.into_iter().map(Into::into).collect(),
        }
    }

    pub(crate) fn constant(value: serde_json::Value) -> Self {
        Self::Constant { value }
    }

    pub(crate) fn preserves_source_sequence(&self) -> bool {
        matches!(self, Self::Unit | Self::Constant { .. })
    }

    pub(crate) fn requires_input(&self) -> bool {
        matches!(self, Self::InputPath { .. })
    }

    pub(crate) fn validate(&self) -> Result<(), ProjectionProtocolError> {
        match self {
            Self::InputPath { path } => {
                if path.is_empty()
                    || path.len() > MAX_PARTITION_PATH_DEPTH
                    || path.iter().any(|segment| {
                        segment.trim().is_empty()
                            || segment.as_bytes().len() > MAX_PARTITION_PATH_SEGMENT_BYTES
                    })
                {
                    return Err(ProjectionProtocolError::InvalidBatch(format!(
                        "projection partition input path must contain 1..={MAX_PARTITION_PATH_DEPTH} non-empty segments of at most {MAX_PARTITION_PATH_SEGMENT_BYTES} bytes"
                    )));
                }
            }
            Self::Constant { value } => {
                let bytes = serde_json::to_vec(value).map_err(|error| {
                    ProjectionProtocolError::InvalidBatch(format!(
                        "projection partition constant cannot be serialized: {error}"
                    ))
                })?;
                if bytes.len() > MAX_PROJECTION_PARTITION_BYTES {
                    return Err(ProjectionProtocolError::InvalidBatch(format!(
                        "projection partition constant exceeds {MAX_PROJECTION_PARTITION_BYTES} canonical JSON bytes"
                    )));
                }
            }
            Self::Unit => {}
        }
        Ok(())
    }

    pub(crate) fn resolve(
        &self,
        canonical_input: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>, ProjectionProtocolError> {
        match self {
            Self::Unit => Ok(None),
            Self::Constant { value } => Ok(Some(value.clone())),
            Self::InputPath { path } => {
                let mut value = canonical_input;
                for segment in path {
                    value = value
                        .as_object()
                        .and_then(|object| object.get(segment))
                        .ok_or_else(|| {
                            ProjectionProtocolError::InvalidBatch(format!(
                                "projection partition input path `{}` is absent",
                                path.join(".")
                            ))
                        })?;
                }
                Ok(Some(value.clone()))
            }
        }
    }
}
