//! Aggregate cell class: one Durable Object analogue per aggregate type,
//! one instance per shard (`{aggregate_type}:{shard}`).

use std::collections::HashMap;

use serde_json::Value;

use super::store::CellStreamStore;
use crate::aggregate::{Aggregate, AggregateRepository};
use crate::microsvc::error::HandlerError;
use crate::microsvc::service::{PortableCommand, Routes};
use crate::microsvc::session::Session;
use crate::repository::{RepositoryError, StreamIdentity};

/// Cell class for aggregate `A`. Equivalent to
/// `#[distributed::cell(aggregate = A)]`: mount the same domain
/// [`PortableCommand`] values used by SOA `Routes::mount`.
///
/// Projectors, GraphQL, and ingest are not methods on this type
/// (`PCH-REQ-005`).
///
/// ```compile_fail
/// fn projectors_are_not_cell_methods<A>(cell: distributed::cell_host::AggregateCell<A>)
/// where
///     A: distributed::Aggregate + Send + Sync + 'static,
/// {
///     let _ = cell.causal_projector;
/// }
/// ```
///
/// ```compile_fail
/// fn graphql_is_not_a_cell_method<A>(cell: distributed::cell_host::AggregateCell<A>)
/// where
///     A: distributed::Aggregate + Send + Sync + 'static,
/// {
///     let _ = cell.bind_graphql;
/// }
/// ```
pub struct AggregateCell<A>
where
    A: Aggregate + Send + Sync + 'static,
{
    shard: StreamIdentity,
    routes: Routes<AggregateRepository<CellStreamStore, A>>,
}

impl<A> AggregateCell<A>
where
    A: Aggregate + Send + Sync + 'static,
{
    /// Open a cell instance addressed as `{aggregate_type}:{shard_id}`.
    pub fn new(shard_id: impl Into<String>) -> Result<Self, RepositoryError> {
        let shard = StreamIdentity::new(A::aggregate_type(), shard_id.into())?;
        let store = CellStreamStore::for_identity(shard.clone());
        Ok(Self {
            shard,
            routes: Routes::from_dependencies(AggregateRepository::new(store)),
        })
    }

    /// Durable Object name: `format!("{}:{}", type, shard)`.
    pub fn instance_name(&self) -> String {
        self.shard.to_string()
    }

    /// Shard id used for cell addressing and SOA `load_by`.
    pub fn shard_id(&self) -> &str {
        self.shard.aggregate_id()
    }

    /// Install a domain command declaration. Same value SOA mounts.
    pub fn mount(
        mut self,
        command: impl PortableCommand<AggregateRepository<CellStreamStore, A>>,
    ) -> Self {
        self.routes = self.routes.mount(command);
        self
    }

    /// Command ids mounted on this cell class instance.
    pub fn command_names(&self) -> Vec<String> {
        self.routes
            .command_specs()
            .unwrap_or_default()
            .into_iter()
            .map(|spec| spec.id)
            .collect()
    }

    /// True when this cell has only command mounts (no projectors/GraphQL services).
    pub fn is_command_only(&self) -> bool {
        self.routes.is_command_only()
    }

    /// Dispatch a mounted command through the cell-local workspace adapter.
    pub async fn dispatch(
        &self,
        command: &str,
        input: Value,
        session: Session,
    ) -> Result<Value, HandlerError> {
        self.routes
            .dispatch_cell_command(command, input, session, &self.shard)
            .await
    }
}

/// Worker-side namespace: `getByName(format!("{}:{}", type, shard))`.
pub struct CellNamespace<A>
where
    A: Aggregate + Send + Sync + 'static,
{
    cells: HashMap<String, AggregateCell<A>>,
}

impl<A> Default for CellNamespace<A>
where
    A: Aggregate + Send + Sync + 'static,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<A> CellNamespace<A>
where
    A: Aggregate + Send + Sync + 'static,
{
    pub fn new() -> Self {
        Self {
            cells: HashMap::new(),
        }
    }

    /// Address a cell by Durable Object name.
    pub fn get_by_name(&self, name: &str) -> Option<&AggregateCell<A>> {
        self.cells.get(name)
    }

    /// Mutable address by Durable Object name.
    pub fn get_by_name_mut(&mut self, name: &str) -> Option<&mut AggregateCell<A>> {
        self.cells.get_mut(name)
    }

    /// Insert a fully mounted cell instance.
    pub fn insert(&mut self, cell: AggregateCell<A>) {
        self.cells.insert(cell.instance_name(), cell);
    }

    /// Create or return the cell for `shard_id`.
    pub fn get_or_create(
        &mut self,
        shard_id: &str,
        mount: impl FnOnce(AggregateCell<A>) -> AggregateCell<A>,
    ) -> Result<&mut AggregateCell<A>, RepositoryError> {
        let name = instance_name::<A>(shard_id);
        if !self.cells.contains_key(&name) {
            let cell = mount(AggregateCell::new(shard_id)?);
            self.cells.insert(name.clone(), cell);
        }
        Ok(self.cells.get_mut(&name).expect("just inserted"))
    }
}

/// Cell instance name: `{aggregate_type}:{shard_id}`.
pub fn instance_name<A: Aggregate>(shard_id: &str) -> String {
    format!("{}:{shard_id}", A::aggregate_type())
}
