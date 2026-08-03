//! Framework-owned local projection mount compilation.
//!
//! Applications declare program descriptors, read models, and epochs. Physical
//! topology, partition codecs, catalog activation, and Surface packaging are
//! derived here.

#![allow(missing_docs)]


use crate::graphql::{SurfaceDirectProjection, SurfaceModeledProjection, SurfaceProjector};
use crate::projection::catalog::{
    ActiveProjectionBindings, ProjectionBindingActivation, ProjectionCatalog,
};
use crate::projection::lower::ProjectionDescriptor;
use crate::projection::placement::{
    ProjectionBinding, ProjectionBindingState, ProjectionEpoch, ProjectionExecutorRoute,
    ProjectionOutput, ProjectionOwner, ProjectionPhysicalTopology, ProjectionSourceBinding,
    PROJECTION_PARTITION_CODEC_VERSION,
};
use crate::projection_protocol::ProjectorTopologyId;
use crate::table::TableSchema;
use crate::RelationalReadModel;

/// One eventual surface projector ready for GraphQL/service mounts.
#[derive(Clone)]
pub struct LocalEventualMount {
    pub owner: String,
    pub projector: SurfaceProjector,
}

/// One direct surface projection ready for GraphQL/service mounts.
#[derive(Clone)]
pub struct LocalDirectMount {
    pub owner: String,
    pub projection: SurfaceDirectProjection,
}

/// Compiled set of local projection mounts for one application.
#[derive(Clone, Default)]
pub struct LocalProjectionMounts {
    pub eventual: Vec<LocalEventualMount>,
    pub direct: Vec<LocalDirectMount>,
}

impl LocalProjectionMounts {
    pub fn projector(&self, owner: &str) -> Option<SurfaceProjector> {
        self.eventual
            .iter()
            .find(|mount| mount.owner == owner)
            .map(|mount| mount.projector.clone())
    }

    pub fn direct_projection(&self, owner: &str) -> Option<SurfaceDirectProjection> {
        self.direct
            .iter()
            .find(|mount| mount.owner == owner)
            .map(|mount| mount.projection.clone())
    }

    pub fn all_owners(&self) -> Vec<crate::graphql::SurfaceProjectionOwner> {
        let mut owners = Vec::new();
        for mount in &self.eventual {
            owners.push(mount.projector.clone().into());
        }
        for mount in &self.direct {
            owners.push(mount.projection.clone().into());
        }
        owners
    }
}

struct PendingEventual {
    owner: String,
    epoch: String,
    binding: ProjectionBinding,
    modeled_factory: Box<
        dyn Fn(
                &ProjectionCatalog,
                &ActiveProjectionBindings,
                &ProjectionBinding,
            ) -> Result<SurfaceModeledProjection, String>
            + Send
            + Sync,
    >,
}

struct PendingDirect {
    owner: String,
    epoch: String,
    binding: ProjectionBinding,
    modeled_factory: Box<
        dyn Fn(
                &ProjectionCatalog,
                &ActiveProjectionBindings,
                &ProjectionBinding,
            ) -> Result<SurfaceModeledProjection, String>
            + Send
            + Sync,
    >,
}

/// Builder that materializes a shared catalog and Surface mounts for local hosting.
pub struct LocalProjectionMountsBuilder {
    service_id: String,
    source: ProjectionSourceBinding,
    eventual: Vec<PendingEventual>,
    direct: Vec<PendingDirect>,
}

impl LocalProjectionMountsBuilder {
    /// `domain_source` is a stable domain event stream name (e.g. `ordered-domain-events`).
    pub fn new(
        service_id: impl Into<String>,
        domain_source: impl Into<String>,
    ) -> Result<Self, String> {
        let service_id = service_id.into();
        let source = ProjectionSourceBinding::try_new(
            format!("{service_id}-domain"),
            domain_source,
            1,
        )
        .map_err(|error| error.to_string())?;
        Ok(Self {
            service_id,
            source,
            eventual: Vec::new(),
            direct: Vec::new(),
        })
    }

    /// Register an eventual projection program targeting model `M`.
    pub fn eventual_model<M, D>(
        mut self,
        owner: impl Into<String>,
        descriptor: ProjectionDescriptor<D>,
        epoch: impl Into<String>,
    ) -> Result<Self, String>
    where
        M: RelationalReadModel,
        D: Copy + 'static,
    {
        let owner = owner.into();
        let epoch = epoch.into();
        let digest = stable_topology_digest(&owner);
        let binding = ProjectionBinding::materialize_eventual(
            descriptor.eventual(),
            self.source.clone(),
            ProjectionOwner::try_new(owner.clone()).map_err(|e| e.to_string())?,
            "distributed-projection-partition",
            PROJECTION_PARTITION_CODEC_VERSION,
            vec![projection_output_for::<M>()?],
            Vec::new(),
            Some(physical_topology(&owner, digest)),
        )
        .map_err(|error| error.to_string())?;
        self.eventual.push(PendingEventual {
            owner,
            epoch,
            binding,
            modeled_factory: Box::new(move |catalog, active, binding| {
                SurfaceModeledProjection::try_from_descriptor(
                    descriptor,
                    catalog,
                    active,
                    binding.id(),
                )
            }),
        });
        Ok(self)
    }

    /// Register a direct projection program targeting model `M`.
    pub fn direct_model<M>(
        mut self,
        owner: impl Into<String>,
        descriptor: ProjectionDescriptor<crate::projection::lower::DirectCandidate>,
        epoch: impl Into<String>,
    ) -> Result<Self, String>
    where
        M: RelationalReadModel,
    {
        let owner = owner.into();
        let epoch = epoch.into();
        let digest = stable_topology_digest(&owner);
        let binding = ProjectionBinding::materialize_direct(
            descriptor.direct(),
            self.source.clone(),
            ProjectionOwner::try_new(owner.clone()).map_err(|e| e.to_string())?,
            "distributed-projection-partition",
            PROJECTION_PARTITION_CODEC_VERSION,
            vec![projection_output_for::<M>()?],
            Vec::new(),
            Some(physical_topology(&owner, digest)),
        )
        .map_err(|error| error.to_string())?;
        self.direct.push(PendingDirect {
            owner,
            epoch,
            binding,
            modeled_factory: Box::new(move |catalog, active, binding| {
                SurfaceModeledProjection::try_from_descriptor(
                    descriptor,
                    catalog,
                    active,
                    binding.id(),
                )
            }),
        });
        Ok(self)
    }

    pub fn build(self) -> Result<LocalProjectionMounts, String> {
        let mut bindings = Vec::new();
        for entry in &self.eventual {
            bindings.push(entry.binding.clone());
        }
        for entry in &self.direct {
            bindings.push(entry.binding.clone());
        }
        let catalog = ProjectionCatalog::try_new(bindings).map_err(|e| e.to_string())?;
        let mut activations = Vec::new();
        for entry in &self.eventual {
            activations.push(activation(&entry.binding, &entry.epoch, &self.service_id)?);
        }
        for entry in &self.direct {
            activations.push(activation(&entry.binding, &entry.epoch, &self.service_id)?);
        }
        let active = catalog
            .activate(activations, None)
            .map_err(|e| e.to_string())?;

        let mut mounts = LocalProjectionMounts::default();
        for entry in &self.eventual {
            let modeled = (entry.modeled_factory)(&catalog, &active, &entry.binding)?;
            mounts.eventual.push(LocalEventualMount {
                owner: entry.owner.clone(),
                projector: SurfaceProjector::new(entry.owner.clone()).modeled(modeled),
            });
        }
        for entry in &self.direct {
            let modeled = (entry.modeled_factory)(&catalog, &active, &entry.binding)?;
            mounts.direct.push(LocalDirectMount {
                owner: entry.owner.clone(),
                projection: SurfaceDirectProjection::new(entry.owner.clone()).modeled(modeled),
            });
        }
        Ok(mounts)
    }
}

fn activation(
    binding: &ProjectionBinding,
    epoch: &str,
    service_id: &str,
) -> Result<ProjectionBindingActivation, String> {
    Ok(ProjectionBindingActivation::new(
        binding.id(),
        binding.program_id(),
        ProjectionEpoch::new(epoch).map_err(|e| e.to_string())?,
        ProjectionBindingState::Active,
        Some(ProjectionExecutorRoute::local(service_id).map_err(|e| e.to_string())?),
    ))
}

fn projection_output_for<M: RelationalReadModel>() -> Result<ProjectionOutput, String> {
    let schema: TableSchema = M::schema().clone();
    ProjectionOutput::try_new(schema.model_name.clone(), schema.table_name.clone(), schema)
        .map_err(|e| e.to_string())
}

fn physical_topology(name: &str, digest: u8) -> ProjectionPhysicalTopology {
    ProjectionPhysicalTopology::from_protocol(
        &ProjectorTopologyId::new(1, name, [digest; 32])
            .expect("canonical local projection topology"),
    )
}

fn stable_topology_digest(owner: &str) -> u8 {
    let mut acc = 0u8;
    for (i, b) in owner.as_bytes().iter().enumerate() {
        acc = acc.wrapping_add(b.wrapping_mul((i as u8).wrapping_add(1)));
    }
    if acc == 0 {
        0x20
    } else {
        acc
    }
}
