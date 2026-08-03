//! Projection ownership catalog for e2e-ui modules.
//!
//! Physical topology/activation is framework-owned material derived from the
//! portable e2e-projections programs. Application modules only receive the
//! resulting Surface mounts.

use distributed::graphql::{SurfaceDirectProjection, SurfaceModeledProjection, SurfaceProjector};
use distributed::projection::catalog::{ProjectionBindingActivation, ProjectionCatalog};
use distributed::projection::lower::ProjectionDescriptor;
use distributed::projection::placement::{
    ProjectionBinding, ProjectionBindingState, ProjectionEpoch, ProjectionExecutorRoute,
    ProjectionOutput, ProjectionOwner, ProjectionPhysicalTopology, ProjectionSourceBinding,
    PROJECTION_PARTITION_CODEC_VERSION,
};
use distributed::projection_protocol::ProjectorTopologyId;
use distributed::RelationalReadModel;
use e2e_projections::{BLOB_GAMES, CHAT_MESSAGES, TODOS};
use e2e_readmodels::{BlobGames, ChatMessages, Todos};

#[derive(Clone)]
pub struct ProjectionOwners {
    pub todo: SurfaceProjector,
    pub chat: SurfaceProjector,
    pub blob: SurfaceDirectProjection,
}

fn projection_output<M: RelationalReadModel>() -> ProjectionOutput {
    let schema = M::schema().clone();
    ProjectionOutput::try_new(schema.model_name.clone(), schema.table_name.clone(), schema)
        .expect("canonical e2e-ui projection output")
}

fn physical_topology(name: &str, digest: u8) -> ProjectionPhysicalTopology {
    ProjectionPhysicalTopology::from_protocol(
        &ProjectorTopologyId::new(1, name, [digest; 32])
            .expect("canonical e2e-ui physical topology"),
    )
}

fn modeled_projection<D>(
    descriptor: ProjectionDescriptor<D>,
    catalog: &ProjectionCatalog,
    active: &distributed::projection::catalog::ActiveProjectionBindings,
    binding: &ProjectionBinding,
) -> SurfaceModeledProjection {
    SurfaceModeledProjection::try_from_descriptor(descriptor, catalog, active, binding.id())
        .expect("modeled projection should resolve through the active catalog")
}

/// Compile the deployment-wide projection catalog and surface mounts once.
pub fn projection_owners() -> ProjectionOwners {
    let source = || {
        ProjectionSourceBinding::try_new("e2e-ui-domain", "ordered-domain-events", 1)
            .expect("canonical e2e-ui domain source")
    };
    let owner = |name| ProjectionOwner::try_new(name).expect("canonical projection owner");

    let todo_binding = ProjectionBinding::materialize_eventual(
        TODOS.eventual(),
        source(),
        owner("project_todos"),
        "distributed-projection-partition",
        PROJECTION_PARTITION_CODEC_VERSION,
        vec![projection_output::<Todos>()],
        Vec::new(),
        Some(physical_topology("project_todos", 0x20)),
    )
    .expect("Todo projection binding");
    let chat_binding = ProjectionBinding::materialize_eventual(
        CHAT_MESSAGES.eventual(),
        source(),
        owner("project_chat_messages"),
        "distributed-projection-partition",
        PROJECTION_PARTITION_CODEC_VERSION,
        vec![projection_output::<ChatMessages>()],
        Vec::new(),
        Some(physical_topology("project_chat_messages", 0x21)),
    )
    .expect("Chat projection binding");
    let blob_binding = ProjectionBinding::materialize_direct(
        BLOB_GAMES.direct(),
        source(),
        owner("project_blob"),
        "distributed-projection-partition",
        PROJECTION_PARTITION_CODEC_VERSION,
        vec![projection_output::<BlobGames>()],
        Vec::new(),
        Some(physical_topology("project_blob", 0x22)),
    )
    .expect("Blob projection binding");

    let catalog = ProjectionCatalog::try_new(vec![
        todo_binding.clone(),
        chat_binding.clone(),
        blob_binding.clone(),
    ])
    .expect("deployment-wide projection catalog");
    let activation = |binding: &ProjectionBinding, epoch: &str| {
        ProjectionBindingActivation::new(
            binding.id(),
            binding.program_id(),
            ProjectionEpoch::new(epoch).expect("canonical projection epoch"),
            ProjectionBindingState::Active,
            Some(
                ProjectionExecutorRoute::local("e2e-ui").expect("canonical local projection route"),
            ),
        )
    };
    let active = catalog
        .activate(
            vec![
                activation(&todo_binding, "e2e-ui-todos-v2"),
                activation(&chat_binding, "e2e-ui-chat-v2"),
                activation(&blob_binding, "e2e-ui-blob-v2"),
            ],
            None,
        )
        .expect("non-overlapping active projection catalog");

    ProjectionOwners {
        todo: SurfaceProjector::new("project_todos").modeled(modeled_projection(
            TODOS,
            &catalog,
            &active,
            &todo_binding,
        )),
        chat: SurfaceProjector::new("project_chat_messages").modeled(modeled_projection(
            CHAT_MESSAGES,
            &catalog,
            &active,
            &chat_binding,
        )),
        blob: SurfaceDirectProjection::new("project_blob").modeled(modeled_projection(
            BLOB_GAMES,
            &catalog,
            &active,
            &blob_binding,
        )),
    }
}
