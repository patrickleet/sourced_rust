use super::constants::{MAX_PARTITION_PATH_DEPTH, MAX_PARTITION_PATH_SEGMENT_BYTES};
use crate::projection_protocol::{ProjectionProtocolError, MAX_PROJECTION_PARTITION_BYTES};
use sha2::{Digest, Sha256};

const MODELED_PARTITION_CONTRACT_DOMAIN: &[u8] =
    b"distributed.modeled-projection-partition-contract.v1";

/// Canonical declaration-owned partition derivation for one projector.
///
/// The runtime evaluates this closed IR from raw JSON before decoding the
/// typed event. It is also part of the compiled topology digest, so changing
/// partition semantics necessarily creates a different durable topology.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum ProjectionPartitionSpec {
    Unit,
    InputPath {
        path: Vec<String>,
    },
    Constant {
        value: serde_json::Value,
    },
    /// Exact deployment binding for a portable non-unit partition.
    ///
    /// This is identity/capability metadata only. Modeled executors resolve
    /// the expression from the typed occurrence plan; legacy raw-input
    /// projector compilation and execution must reject this form.
    ModeledExpression {
        expression: serde_json::Value,
        codec: String,
        codec_version: u16,
        digest: String,
    },
    /// A modeled owner retained only for draining work has no active contract
    /// from which new query, live, or command capability may be minted.
    ModeledInactive,
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

    pub(crate) fn modeled_expression(
        expression: serde_json::Value,
        codec: impl Into<String>,
        codec_version: u16,
    ) -> Result<Self, ProjectionProtocolError> {
        let codec = codec.into();
        let digest = modeled_partition_digest(&expression, &codec, codec_version)?;
        let partition = Self::ModeledExpression {
            expression,
            codec,
            codec_version,
            digest,
        };
        partition.validate()?;
        Ok(partition)
    }

    pub(crate) fn modeled_inactive() -> Self {
        Self::ModeledInactive
    }

    pub(crate) fn is_modeled_only(&self) -> bool {
        matches!(self, Self::ModeledExpression { .. } | Self::ModeledInactive)
    }

    pub(crate) fn preserves_source_sequence(&self) -> bool {
        matches!(self, Self::Unit | Self::Constant { .. })
    }

    pub(crate) fn requires_input(&self) -> bool {
        matches!(
            self,
            Self::InputPath { .. } | Self::ModeledExpression { .. } | Self::ModeledInactive
        )
    }

    pub(crate) fn validate(&self) -> Result<(), ProjectionProtocolError> {
        match self {
            Self::InputPath { path } => {
                if path.is_empty()
                    || path.len() > MAX_PARTITION_PATH_DEPTH
                    || path.iter().any(|segment| {
                        segment.trim().is_empty()
                            || segment.len() > MAX_PARTITION_PATH_SEGMENT_BYTES
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
            Self::ModeledExpression {
                expression,
                codec,
                codec_version,
                digest,
            } => {
                let expression_kind = expression
                    .as_object()
                    .and_then(|object| object.get("kind"))
                    .and_then(serde_json::Value::as_str);
                if expression_kind != Some("expression") {
                    return Err(ProjectionProtocolError::InvalidBatch(
                        "modeled projection partition must retain a non-unit portable expression"
                            .into(),
                    ));
                }
                let expected = modeled_partition_digest(expression, codec, *codec_version)?;
                if digest != &expected {
                    return Err(ProjectionProtocolError::InvalidBatch(
                        "modeled projection partition contract digest mismatch".into(),
                    ));
                }
            }
            Self::Unit | Self::ModeledInactive => {}
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
            Self::ModeledExpression { .. } | Self::ModeledInactive => {
                Err(ProjectionProtocolError::InvalidBatch(
                    "modeled projection partitions must be resolved from the typed occurrence plan"
                        .into(),
                ))
            }
        }
    }
}

fn modeled_partition_digest(
    expression: &serde_json::Value,
    codec: &str,
    codec_version: u16,
) -> Result<String, ProjectionProtocolError> {
    if codec.trim().is_empty() || codec_version == 0 {
        return Err(ProjectionProtocolError::InvalidBatch(
            "modeled projection partition codec must be non-empty and versioned".into(),
        ));
    }
    let canonical = canonical_json(expression);
    let bytes = serde_json::to_vec(&canonical).map_err(|error| {
        ProjectionProtocolError::InvalidBatch(format!(
            "modeled projection partition expression cannot be serialized: {error}"
        ))
    })?;
    if bytes.len() > MAX_PROJECTION_PARTITION_BYTES {
        return Err(ProjectionProtocolError::InvalidBatch(format!(
            "modeled projection partition expression exceeds {MAX_PROJECTION_PARTITION_BYTES} canonical JSON bytes"
        )));
    }
    let mut digest = Sha256::new();
    digest.update(MODELED_PARTITION_CONTRACT_DOMAIN);
    digest.update(codec.as_bytes());
    digest.update(codec_version.to_be_bytes());
    digest.update(bytes);
    Ok(format!(
        "sha256:{}",
        digest
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    ))
}

fn canonical_json(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.iter().map(canonical_json).collect())
        }
        serde_json::Value::Object(object) => {
            let mut entries = object.iter().collect::<Vec<_>>();
            entries.sort_by_key(|(key, _)| *key);
            serde_json::Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key.clone(), canonical_json(value)))
                    .collect(),
            )
        }
        scalar => scalar.clone(),
    }
}
