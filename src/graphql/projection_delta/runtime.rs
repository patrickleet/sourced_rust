use std::sync::Arc;

use serde::Serialize;

use super::authorization::{
    ProjectionPartitionAuthority, ProjectionPartitionScopeEncoder, ProjectionVisibilityEvaluator,
    SelectedSurfacePolicyVisibility,
};
use super::lower::ProjectionDeltaRequestAuthority;
use super::types::{
    AuthorizationTransition, ProjectionDeltaCacheScopeToken, ProjectionMutationSource,
};
use super::{
    ProjectionDelta, ProjectionDeltaError, ProjectionDeltaMutation, ProjectionDeltaPartition,
    ProjectionDeltaScope, ProjectionDeltaSurfaceIdentity,
};
use crate::command_ledger::{CausationId, PrincipalPartitionId};
use crate::graphql::client_manifest::DistributedClientSurfaceExport;
use crate::graphql::protocol::{
    CommandProjectionMetadataV1, CommandProjectionObligationV1, OpaqueProtocolToken,
    ProtocolTokenCodec, ProtocolTokenError, ProtocolTokenPurpose,
};
use crate::graphql::surface::Surface;
use crate::{
    ResolvedProjectionMutation, ResolvedProjectionPartition, ResolvedProjectionRelationshipEffect,
};

/// Production request authority for actual role-safe projection lowering.
///
/// Construction seals the exact selected Surface, verified principal
/// partition, authorization generation, cache scope, causation, keyed codec,
/// and lifetime. No application-provided logical partition or wire identity
/// can be substituted later.
pub(crate) struct ProtocolProjectionDeltaRequestAuthority {
    export: DistributedClientSurfaceExport,
    surface: Arc<Surface>,
    codec: ProtocolTokenCodec,
    principal_scope: PrincipalPartitionId,
    authorization_generation: String,
    cache_scope: ProjectionDeltaCacheScopeToken,
    causation_id: CausationId,
    issued_at_unix_ms: u64,
    expires_at_unix_ms: u64,
}

impl ProtocolProjectionDeltaRequestAuthority {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn try_new(
        export: DistributedClientSurfaceExport,
        codec: ProtocolTokenCodec,
        principal_scope: PrincipalPartitionId,
        authorization_generation: impl Into<String>,
        cache_scope: &OpaqueProtocolToken,
        causation_id: CausationId,
        issued_at_unix_ms: u64,
        expires_at_unix_ms: u64,
    ) -> Result<Self, ProjectionRuntimeAuthorityError> {
        let authorization_generation = authorization_generation.into();
        if authorization_generation.trim().is_empty() || issued_at_unix_ms >= expires_at_unix_ms {
            return Err(ProjectionRuntimeAuthorityError::InvalidAuthority);
        }
        export
            .manifest()
            .map_err(|_| ProjectionRuntimeAuthorityError::InvalidAuthority)?;
        let surface = export.surface().clone();
        SelectedSurfacePolicyVisibility::try_new(&surface)
            .map_err(|_| ProjectionRuntimeAuthorityError::InvalidAuthority)?;
        let cache_scope = ProjectionDeltaCacheScopeToken::from_protocol(cache_scope)
            .map_err(|_| ProjectionRuntimeAuthorityError::InvalidAuthority)?;
        Ok(Self {
            export,
            surface,
            codec,
            principal_scope,
            authorization_generation,
            cache_scope,
            causation_id,
            issued_at_unix_ms,
            expires_at_unix_ms,
        })
    }

    pub(crate) fn export(&self) -> &DistributedClientSurfaceExport {
        &self.export
    }

    /// Derive canonical obligations only for finite, observable targets.
    ///
    /// Invalidations and recovery-only paths request revalidation but never
    /// invent a record obligation.
    pub(crate) fn metadata(
        &self,
        delta: ProjectionDelta,
    ) -> Result<CommandProjectionMetadataV1, ProjectionRuntimeAuthorityError> {
        delta
            .validate_replay_scope(
                &self
                    .export
                    .manifest()
                    .map_err(|_| ProjectionRuntimeAuthorityError::InvalidAuthority)?,
                self,
            )
            .map_err(ProjectionRuntimeAuthorityError::Delta)?;

        let mut obligations = Vec::new();
        for operation in &delta.operations {
            let Some(scope) = ObservableScope::from_mutation(&operation.mutation) else {
                continue;
            };
            for projection_ref in &operation.projection_refs {
                let projection = delta
                    .projections
                    .get(*projection_ref as usize)
                    .ok_or(ProjectionRuntimeAuthorityError::InvalidAuthority)?;
                let token = self
                    .codec
                    .issue(
                        ProtocolTokenPurpose::ProjectionObligation,
                        &ObligationTokenMaterial {
                            domain: "distributed.graphql.command-projection-obligation",
                            version: 1,
                            principal_scope: self.principal_scope.as_str(),
                            surface: &delta.identity.surface,
                            authorization_generation: &self.authorization_generation,
                            cache_scope: self.cache_scope.as_str(),
                            causation_id: self.causation_id.as_str(),
                            expires_at_unix_ms: self.expires_at_unix_ms,
                            projection,
                            scope: &scope,
                        },
                    )
                    .map_err(ProjectionRuntimeAuthorityError::Token)?;
                obligations.push(CommandProjectionObligationV1 {
                    projection_ref: *projection_ref,
                    scope_token: token,
                });
            }
        }
        let revalidate = !delta.recoveries.is_empty()
            || delta.operations.iter().any(|operation| {
                matches!(
                    operation.mutation,
                    ProjectionDeltaMutation::InvalidateModel { .. }
                        | ProjectionDeltaMutation::InvalidateRelationship { .. }
                )
            });
        CommandProjectionMetadataV1::try_new(
            self.issued_at_unix_ms,
            self.expires_at_unix_ms,
            delta,
            obligations,
            revalidate,
        )
        .map_err(|_| ProjectionRuntimeAuthorityError::InvalidMetadata)
    }

    pub(crate) fn verify_partition_token(
        &self,
        token: &str,
        surface: &ProjectionDeltaSurfaceIdentity,
        partition: &ResolvedProjectionPartition,
        now_unix_ms: u64,
    ) -> Result<(), ProjectionRuntimeAuthorityError> {
        if now_unix_ms >= self.expires_at_unix_ms {
            return Err(ProjectionRuntimeAuthorityError::Expired);
        }
        let token =
            OpaqueProtocolToken::parse(token).map_err(ProjectionRuntimeAuthorityError::Token)?;
        self.codec
            .verify(
                &token,
                ProtocolTokenPurpose::ProjectionPartition,
                &PartitionTokenMaterial {
                    domain: "distributed.graphql.projection-partition",
                    version: 1,
                    principal_scope: self.principal_scope.as_str(),
                    surface,
                    authorization_generation: &self.authorization_generation,
                    cache_scope: self.cache_scope.as_str(),
                    expires_at_unix_ms: self.expires_at_unix_ms,
                    canonical_partition: partition.canonical_bytes(),
                },
            )
            .map_err(ProjectionRuntimeAuthorityError::Token)
    }
}

impl ProjectionPartitionScopeEncoder for ProtocolProjectionDeltaRequestAuthority {
    fn encode(
        &self,
        authority: ProjectionPartitionAuthority<'_>,
        partition: &ResolvedProjectionPartition,
    ) -> Result<Option<ProjectionDeltaPartition>, ProjectionDeltaError> {
        if authority.principal_scope != &self.principal_scope
            || authority.authorization_generation != self.authorization_generation
            || authority.cache_scope != &self.cache_scope
        {
            return Err(ProjectionDeltaError::AuthorizationMapping);
        }
        let token = self
            .codec
            .issue(
                ProtocolTokenPurpose::ProjectionPartition,
                &PartitionTokenMaterial {
                    domain: "distributed.graphql.projection-partition",
                    version: 1,
                    principal_scope: self.principal_scope.as_str(),
                    surface: authority.surface,
                    authorization_generation: &self.authorization_generation,
                    cache_scope: self.cache_scope.as_str(),
                    expires_at_unix_ms: self.expires_at_unix_ms,
                    canonical_partition: partition.canonical_bytes(),
                },
            )
            .map_err(|_| ProjectionDeltaError::AuthorizationMapping)?;
        Ok(Some(ProjectionDeltaPartition::Opaque {
            token: token.as_str().to_owned(),
        }))
    }
}

impl ProjectionVisibilityEvaluator for ProtocolProjectionDeltaRequestAuthority {
    fn record_transition(
        &self,
        source: ProjectionMutationSource,
        mutation: &ResolvedProjectionMutation,
    ) -> Result<AuthorizationTransition, ProjectionDeltaError> {
        SelectedSurfacePolicyVisibility::try_new(&self.surface)?.record_transition(source, mutation)
    }

    fn relationship_transition(
        &self,
        source: ProjectionMutationSource,
        effect: &ResolvedProjectionRelationshipEffect,
    ) -> Result<AuthorizationTransition, ProjectionDeltaError> {
        SelectedSurfacePolicyVisibility::try_new(&self.surface)?
            .relationship_transition(source, effect)
    }
}

impl ProjectionDeltaRequestAuthority for ProtocolProjectionDeltaRequestAuthority {
    fn authorization_generation(&self) -> &str {
        &self.authorization_generation
    }

    fn principal_scope(&self) -> &PrincipalPartitionId {
        &self.principal_scope
    }

    fn cache_scope(&self) -> &ProjectionDeltaCacheScopeToken {
        &self.cache_scope
    }

    fn command_causation_id(&self) -> &CausationId {
        &self.causation_id
    }
}

#[derive(Serialize)]
struct PartitionTokenMaterial<'a> {
    domain: &'static str,
    version: u16,
    principal_scope: &'a str,
    surface: &'a ProjectionDeltaSurfaceIdentity,
    authorization_generation: &'a str,
    cache_scope: &'a str,
    expires_at_unix_ms: u64,
    canonical_partition: &'a [u8],
}

#[derive(Serialize)]
struct ObligationTokenMaterial<'a> {
    domain: &'static str,
    version: u16,
    principal_scope: &'a str,
    surface: &'a ProjectionDeltaSurfaceIdentity,
    authorization_generation: &'a str,
    cache_scope: &'a str,
    causation_id: &'a str,
    expires_at_unix_ms: u64,
    projection: &'a super::ProjectionDeltaProjectionIdentity,
    scope: &'a ObservableScope<'a>,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ObservableScope<'a> {
    Record {
        scope: &'a ProjectionDeltaScope,
    },
    Edge {
        relationship: &'a str,
        source: &'a ProjectionDeltaScope,
        target: &'a ProjectionDeltaScope,
    },
}

impl<'a> ObservableScope<'a> {
    fn from_mutation(mutation: &'a ProjectionDeltaMutation) -> Option<Self> {
        match mutation {
            ProjectionDeltaMutation::Upsert { scope, .. }
            | ProjectionDeltaMutation::Patch { scope, .. }
            | ProjectionDeltaMutation::Delete { scope } => Some(Self::Record { scope }),
            ProjectionDeltaMutation::Link {
                relationship,
                source,
                target,
            }
            | ProjectionDeltaMutation::Unlink {
                relationship,
                source,
                target,
            } => Some(Self::Edge {
                relationship,
                source,
                target,
            }),
            ProjectionDeltaMutation::InvalidateModel { .. }
            | ProjectionDeltaMutation::InvalidateRelationship { .. } => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ProjectionRuntimeAuthorityError {
    InvalidAuthority,
    Expired,
    Delta(ProjectionDeltaError),
    Token(ProtocolTokenError),
    InvalidMetadata,
}

impl std::fmt::Display for ProjectionRuntimeAuthorityError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidAuthority => "invalid projection request authority",
            Self::Expired => "projection request authority has expired",
            Self::Delta(_) => "projection delta validation failed",
            Self::Token(_) => "projection scope token validation failed",
            Self::InvalidMetadata => "command projection metadata is invalid",
        })
    }
}

impl std::error::Error for ProjectionRuntimeAuthorityError {}
