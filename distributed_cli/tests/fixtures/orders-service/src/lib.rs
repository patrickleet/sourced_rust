//! Minimal Distributed service fixture for `dctl` manifest-harness integration
//! tests: one read model (→ an `orders` table) registered in the project manifest.

use std::any::TypeId;

use distributed::graphql::{
    build_surface, surface_for_role, typed_command, DistributedClientSurfaceExport,
    Atomic, GraphqlInputType, GraphqlOutputType, GraphqlTypeDef, GraphqlTypeField, PreparedCommand,
    RoleGrant, SurfaceOptions, SurfaceProjector,
};
use distributed::microsvc::{CausalCommandContext, HandlerError, Routes, Service};
use distributed::{
    Aggregate, AggregateRepository, DistributedProjectManifest, Entity, EventRecord,
    InMemoryRepository, ReadModel,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ReadModel)]
#[table("orders")]
pub struct OrderView {
    #[id("order_id")]
    pub order_id: String,
    pub status: String,
}

impl GraphqlOutputType for OrderView {
    fn graphql_type() -> GraphqlTypeDef {
        GraphqlTypeDef::new(
            "OrderView",
            vec![
                GraphqlTypeField {
                    name: "order_id".into(),
                    type_name: "String".into(),
                    nullable: false,
                    list: false,
                    item_nullable: false,
                    nested: None,
                },
                GraphqlTypeField {
                    name: "status".into(),
                    type_name: "String".into(),
                    nullable: false,
                    list: false,
                    item_nullable: false,
                    nested: None,
                },
            ],
        )
        .with_type_id(TypeId::of::<Self>())
    }
}

#[derive(Deserialize)]
struct ProjectOrderInput {
    order_id: String,
}

impl GraphqlInputType for ProjectOrderInput {
    fn graphql_type() -> GraphqlTypeDef {
        GraphqlTypeDef::new(
            "ProjectOrderInput",
            vec![GraphqlTypeField {
                name: "order_id".into(),
                type_name: "String".into(),
                nullable: false,
                list: false,
                item_nullable: false,
                nested: None,
            }],
        )
        .with_type_id(TypeId::of::<Self>())
    }
}

#[derive(Default)]
struct FixtureAggregate {
    entity: Entity,
}

impl Aggregate for FixtureAggregate {
    type ReplayError = std::convert::Infallible;

    fn entity(&self) -> &Entity {
        &self.entity
    }

    fn entity_mut(&mut self) -> &mut Entity {
        &mut self.entity
    }

    fn replay_event(&mut self, _event: &EventRecord) -> Result<(), Self::ReplayError> {
        Ok(())
    }
}

async fn project_order(
    _context: &CausalCommandContext<'_, FixtureAggregate>,
    _input: ProjectOrderInput,
) -> Result<PreparedCommand<Atomic<OrderView>>, HandlerError> {
    Err(HandlerError::Rejected(
        "manifest-only fixture does not execute commands".into(),
    ))
}

/// The entrypoint `dctl describe`/`dctl schema` call by default
/// (`<crate>::distributed_manifest`).
pub fn distributed_manifest() -> DistributedProjectManifest {
    DistributedProjectManifest::new("orders").read_model::<OrderView>()
}

/// Pool-free client export used by `dctl client-manifest`. Both the CLI harness
/// and a runtime engine finish through `DistributedClientSurfaceExport::manifest`.
pub fn distributed_client_surface() -> DistributedClientSurfaceExport {
    let project = distributed_manifest();
    let service = Service::new().named("orders").routes(
        Routes::new()
            .with_repo(AggregateRepository::<_, FixtureAggregate>::new(
                InMemoryRepository::new(),
            ))
            .typed_command(
                typed_command::<ProjectOrderInput, Atomic<OrderView>>("order.project")
                    .field_name("orders_project")
                    .roles(["user"]),
            )
            .handle(project_order),
    );
    let full = build_surface(&project.tables, &SurfaceOptions::sqlite())
        .expect("fixture Surface should build")
        .with_service(&service)
        .expect("fixture typed service should bind")
        .with_projectors([SurfaceProjector::new("project_orders")
            .facts(["order.projected"])
            .models(["OrderView"])
            .change_epoch("orders-v1")])
        .expect("fixture projector should bind");
    let grants = std::collections::BTreeMap::from([(
        "OrderView".to_string(),
        RoleGrant::all_columns().with_aggregations(),
    )]);
    let user =
        surface_for_role(&full, "user", &grants).expect("fixture role policy should be valid");
    DistributedClientSurfaceExport::from_project(&project, user)
        .expect("fixture Surface should be role-selected")
}
