//! Minimal Distributed service fixture for `dctl` manifest-harness integration
//! tests: one read model (→ an `orders` table) registered in the project manifest.

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
