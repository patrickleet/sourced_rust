use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::OpaqueProtocolToken;
use crate::graphql::projection_delta::{ProjectionDelta, ProjectionDeltaError};
use crate::MAX_DOMAIN_EVENT_BODY_BYTES;

/// Version of the command response/status projection metadata envelope.
pub(crate) const COMMAND_PROJECTION_METADATA_WIRE_VERSION: u16 = 1;
/// Maximum exact causal obligations carried by one command.
pub(crate) const MAX_COMMAND_PROJECTION_OBLIGATIONS: usize = 128;

/// One exact selected projector obligation.
///
/// `projection_ref` indexes the role-safe delta projection inventory. The
/// scope remains an authenticated opaque token; model keys, logical
/// partitions, physical plans, and raw events are never persisted here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CommandProjectionObligationV1 {
    pub(crate) projection_ref: u32,
    pub(crate) scope_token: OpaqueProtocolToken,
}

/// Replay-stable command projection metadata persisted atomically with the
/// terminal command outcome.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CommandProjectionMetadataV1 {
    pub(crate) wire_version: u16,
    pub(crate) issued_at_unix_ms: u64,
    pub(crate) expires_at_unix_ms: u64,
    pub(crate) delta: ProjectionDelta,
    pub(crate) obligations: Vec<CommandProjectionObligationV1>,
    pub(crate) revalidate: bool,
}

impl CommandProjectionMetadataV1 {
    pub(crate) fn try_new(
        issued_at_unix_ms: u64,
        expires_at_unix_ms: u64,
        delta: ProjectionDelta,
        mut obligations: Vec<CommandProjectionObligationV1>,
        revalidate: bool,
    ) -> Result<Self, CommandProjectionMetadataError> {
        obligations.sort_by(|left, right| {
            left.projection_ref
                .cmp(&right.projection_ref)
                .then_with(|| left.scope_token.as_str().cmp(right.scope_token.as_str()))
        });
        obligations.dedup();
        let metadata = Self {
            wire_version: COMMAND_PROJECTION_METADATA_WIRE_VERSION,
            issued_at_unix_ms,
            expires_at_unix_ms,
            delta,
            obligations,
            revalidate,
        };
        metadata.validate()?;
        Ok(metadata)
    }

    pub(crate) fn from_json(bytes: &[u8]) -> Result<Self, CommandProjectionMetadataError> {
        if bytes.len() > MAX_DOMAIN_EVENT_BODY_BYTES {
            return Err(CommandProjectionMetadataError::BodyTooLarge {
                len: bytes.len(),
                max: MAX_DOMAIN_EVENT_BODY_BYTES,
            });
        }
        let metadata = serde_json::from_slice::<Self>(bytes)
            .map_err(|error| CommandProjectionMetadataError::InvalidWire(error.to_string()))?;
        metadata.validate()?;
        if metadata.canonical_bytes()? != bytes {
            return Err(CommandProjectionMetadataError::NonCanonical);
        }
        Ok(metadata)
    }

    pub(crate) fn canonical_bytes(&self) -> Result<Vec<u8>, CommandProjectionMetadataError> {
        self.validate()?;
        let bytes = serde_json::to_vec(self)
            .map_err(|error| CommandProjectionMetadataError::InvalidWire(error.to_string()))?;
        if bytes.len() > MAX_DOMAIN_EVENT_BODY_BYTES {
            return Err(CommandProjectionMetadataError::BodyTooLarge {
                len: bytes.len(),
                max: MAX_DOMAIN_EVENT_BODY_BYTES,
            });
        }
        Ok(bytes)
    }

    pub(crate) fn validate_not_expired(
        &self,
        now_unix_ms: u64,
    ) -> Result<(), CommandProjectionMetadataError> {
        self.validate()?;
        if now_unix_ms >= self.expires_at_unix_ms {
            return Err(CommandProjectionMetadataError::Expired);
        }
        Ok(())
    }

    fn validate(&self) -> Result<(), CommandProjectionMetadataError> {
        if self.wire_version != COMMAND_PROJECTION_METADATA_WIRE_VERSION {
            return Err(CommandProjectionMetadataError::UnsupportedVersion(
                self.wire_version,
            ));
        }
        if self.issued_at_unix_ms >= self.expires_at_unix_ms {
            return Err(CommandProjectionMetadataError::InvalidLifetime);
        }
        self.delta
            .canonical_bytes()
            .map_err(CommandProjectionMetadataError::Delta)?;
        if self.obligations.len() > MAX_COMMAND_PROJECTION_OBLIGATIONS {
            return Err(CommandProjectionMetadataError::TooManyObligations {
                len: self.obligations.len(),
                max: MAX_COMMAND_PROJECTION_OBLIGATIONS,
            });
        }
        if !self.revalidate && !self.delta.recoveries.is_empty() {
            return Err(CommandProjectionMetadataError::MissingRevalidation);
        }

        let operation_projection_refs = self
            .delta
            .operations
            .iter()
            .flat_map(|operation| operation.projection_refs.iter().copied())
            .collect::<BTreeSet<_>>();
        let mut previous: Option<(u32, &str)> = None;
        for obligation in &self.obligations {
            if obligation.projection_ref as usize >= self.delta.projections.len()
                || !operation_projection_refs.contains(&obligation.projection_ref)
            {
                return Err(CommandProjectionMetadataError::UnknownProjectionReference);
            }
            let parsed = OpaqueProtocolToken::parse(obligation.scope_token.as_str())
                .map_err(|_| CommandProjectionMetadataError::InvalidScopeToken)?;
            if parsed.as_str().split('.').nth(1) != Some("projection-obligation") {
                return Err(CommandProjectionMetadataError::InvalidScopeToken);
            }
            let current = (obligation.projection_ref, obligation.scope_token.as_str());
            if previous.is_some_and(|previous| previous >= current) {
                return Err(CommandProjectionMetadataError::NonCanonical);
            }
            previous = Some(current);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CommandProjectionMetadataError {
    InvalidWire(String),
    UnsupportedVersion(u16),
    InvalidLifetime,
    Expired,
    Delta(ProjectionDeltaError),
    TooManyObligations { len: usize, max: usize },
    UnknownProjectionReference,
    InvalidScopeToken,
    MissingRevalidation,
    NonCanonical,
    BodyTooLarge { len: usize, max: usize },
}

impl std::fmt::Display for CommandProjectionMetadataError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidWire(error) => {
                write!(formatter, "invalid command projection metadata: {error}")
            }
            Self::UnsupportedVersion(version) => {
                write!(
                    formatter,
                    "unsupported command projection metadata version `{version}`"
                )
            }
            Self::InvalidLifetime => {
                formatter.write_str("command projection metadata lifetime is invalid")
            }
            Self::Expired => formatter.write_str("command projection metadata has expired"),
            Self::Delta(error) => error.fmt(formatter),
            Self::TooManyObligations { len, max } => write!(
                formatter,
                "command projection metadata has {len} obligations, exceeding {max}"
            ),
            Self::UnknownProjectionReference => formatter
                .write_str("command projection obligation references an unknown projection"),
            Self::InvalidScopeToken => {
                formatter.write_str("command projection obligation scope token is invalid")
            }
            Self::MissingRevalidation => {
                formatter.write_str("command projection recovery requires explicit revalidation")
            }
            Self::NonCanonical => {
                formatter.write_str("command projection metadata is not canonical")
            }
            Self::BodyTooLarge { len, max } => write!(
                formatter,
                "command projection metadata is {len} bytes, exceeding {max}"
            ),
        }
    }
}

impl std::error::Error for CommandProjectionMetadataError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graphql::client_manifest::{
        DISTRIBUTED_CLIENT_MANIFEST_VERSION, DISTRIBUTED_CLIENT_PROTOCOL_VERSION,
    };
    use crate::graphql::projection_delta::{
        DeltaKeyField, DeltaValue, ProjectionDeltaIdentity, ProjectionDeltaMutation,
        ProjectionDeltaOccurrence, ProjectionDeltaOperation, ProjectionDeltaPartition,
        ProjectionDeltaProjectionIdentity, ProjectionDeltaScope, ProjectionDeltaSurfaceIdentity,
        PROJECTION_DELTA_WIRE_VERSION,
    };
    use crate::graphql::protocol::{ProtocolTokenCodec, ProtocolTokenPurpose};

    fn delta() -> ProjectionDelta {
        ProjectionDelta {
            wire_version: PROJECTION_DELTA_WIRE_VERSION,
            identity: ProjectionDeltaIdentity {
                manifest_version: DISTRIBUTED_CLIENT_MANIFEST_VERSION,
                client_protocol_version: DISTRIBUTED_CLIENT_PROTOCOL_VERSION,
                surface: ProjectionDeltaSurfaceIdentity::Role {
                    name: "member".into(),
                },
                schema_fingerprint: "sha256:schema".into(),
                protocol_fingerprint: "sha256:protocol".into(),
                authorization_generation: "auth-generation-1".into(),
                cache_scope_token: "v1.cache-scope.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
                    .into(),
                command_causation_id: "cause-1".into(),
            },
            projections: vec![ProjectionDeltaProjectionIdentity {
                program_id:
                    "pp1:sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                        .into(),
                binding_id:
                    "pb1:sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                        .into(),
                epoch: "todos-v1".into(),
                program_ir_version: crate::projection::PROJECTION_PROGRAM_IR_VERSION,
                operation_semantics_version:
                    crate::projection::PROJECTION_OPERATION_SEMANTICS_VERSION,
            }],
            occurrences: vec![ProjectionDeltaOccurrence {
                causation_id: "cause-1".into(),
                ordinal: 0,
                occurrence_id: "occurrence-1".into(),
            }],
            operations: vec![ProjectionDeltaOperation {
                occurrence_ordinal: 0,
                projection_refs: vec![0],
                mutation: ProjectionDeltaMutation::Delete {
                    scope: ProjectionDeltaScope {
                        partition: ProjectionDeltaPartition::Unit,
                        model: "Todos".into(),
                        key: vec![DeltaKeyField {
                            ordinal: 0,
                            field: "todo_id".into(),
                            value: DeltaValue::String("todo-1".into()),
                        }],
                    },
                },
            }],
            recoveries: Vec::new(),
        }
    }

    #[test]
    fn metadata_round_trips_canonical_bytes_and_deduplicates_scope_tokens() {
        let codec = ProtocolTokenCodec::new([0x31; 32]);
        let token = codec
            .issue(ProtocolTokenPurpose::ProjectionObligation, &("scope", 1))
            .unwrap();
        let metadata = CommandProjectionMetadataV1::try_new(
            100,
            200,
            delta(),
            vec![
                CommandProjectionObligationV1 {
                    projection_ref: 0,
                    scope_token: token.clone(),
                },
                CommandProjectionObligationV1 {
                    projection_ref: 0,
                    scope_token: token,
                },
            ],
            false,
        )
        .unwrap();
        assert_eq!(metadata.obligations.len(), 1);
        let bytes = metadata.canonical_bytes().unwrap();
        assert_eq!(
            CommandProjectionMetadataV1::from_json(&bytes).unwrap(),
            metadata
        );
        assert_eq!(
            metadata.validate_not_expired(200),
            Err(CommandProjectionMetadataError::Expired)
        );
    }

    #[test]
    fn metadata_accepts_128_obligations_and_rejects_129_before_completion() {
        let codec = ProtocolTokenCodec::new([0x32; 32]);
        let obligations = (0..=MAX_COMMAND_PROJECTION_OBLIGATIONS)
            .map(|index| CommandProjectionObligationV1 {
                projection_ref: 0,
                scope_token: codec
                    .issue(
                        ProtocolTokenPurpose::ProjectionObligation,
                        &("scope", index),
                    )
                    .unwrap(),
            })
            .collect::<Vec<_>>();
        CommandProjectionMetadataV1::try_new(
            100,
            200,
            delta(),
            obligations[..MAX_COMMAND_PROJECTION_OBLIGATIONS].to_vec(),
            false,
        )
        .unwrap();
        assert!(matches!(
            CommandProjectionMetadataV1::try_new(100, 200, delta(), obligations, false),
            Err(CommandProjectionMetadataError::TooManyObligations { len: 129, max: 128 })
        ));
    }
}
