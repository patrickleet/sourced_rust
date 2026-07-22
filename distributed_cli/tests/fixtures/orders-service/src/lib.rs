//! Minimal Distributed service fixture for `dctl` manifest-harness integration
//! tests: one read model (→ an `orders` table) registered in the project manifest.

use distributed::graphql::{
    build_surface, surface_for_role, DistributedClientSurfaceExport, RoleGrant, SurfaceOptions,
};
use distributed::{DistributedProjectManifest, ReadModel};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ReadModel)]
#[table("orders")]
pub struct OrderView {
    #[id("order_id")]
    pub order_id: String,
    pub status: String,
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
    let full = build_surface(&project.tables, &SurfaceOptions::sqlite())
        .expect("fixture Surface should build");
    let grants = std::collections::BTreeMap::from([(
        "OrderView".to_string(),
        RoleGrant::all_columns().with_aggregations(),
    )]);
    let user =
        surface_for_role(&full, "user", &grants).expect("fixture role policy should be valid");
    DistributedClientSurfaceExport::from_project(&project, user)
        .expect("fixture Surface should be role-selected")
}
