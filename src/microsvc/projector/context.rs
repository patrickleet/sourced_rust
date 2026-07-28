use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use crate::bus::Message;
use crate::projection_protocol::{
    ProjectionExecutionSnapshotBatch, ProjectionExecutionSnapshotBatchRequest,
    ProjectionGraphSnapshot, ProjectionGraphSnapshotRequest, ProjectionProtocolError,
    ProjectionProtocolStore, ProjectionQuerySnapshot, ProjectionQuerySnapshotRequest,
    ProjectionRecordScope, ProjectionWorkspace, RecordRevision, MAX_PROJECTION_QUERY_BATCH_ROWS,
};
use crate::read_model::RelationalReadModel;
use crate::table::{key_from_row, RowKey, RowPatch};

use super::super::HandlerError;
use super::graph_workspace::ProjectionReadModelWorkspace;

#[derive(Default)]
pub(super) struct ProjectionQueryScopeBudget {
    scopes: Mutex<HashSet<ProjectionRecordScope>>,
}

impl ProjectionQueryScopeBudget {
    pub(super) fn reserve<'a>(
        &self,
        scopes: impl IntoIterator<Item = &'a ProjectionRecordScope>,
    ) -> Result<(), ProjectionProtocolError> {
        let mut current = self.scopes.lock().map_err(|_| {
            ProjectionProtocolError::InvalidBatch(
                "projection query-scope budget is unavailable".into(),
            )
        })?;
        let additions = scopes
            .into_iter()
            .filter(|scope| !current.contains(*scope))
            .cloned()
            .collect::<HashSet<_>>();
        let next_len = current.len().checked_add(additions.len()).ok_or_else(|| {
            ProjectionProtocolError::InvalidBatch(
                "projection context query-scope count overflowed".into(),
            )
        })?;
        if next_len > MAX_PROJECTION_QUERY_BATCH_ROWS {
            return Err(ProjectionProtocolError::InvalidBatch(format!(
                "projection context has {next_len} unique query scopes; maximum is {MAX_PROJECTION_QUERY_BATCH_ROWS}"
            )));
        }
        current.extend(additions);
        Ok(())
    }

    pub(super) fn reserve_graph_root(
        &self,
        root: &ProjectionRecordScope,
    ) -> Result<usize, ProjectionProtocolError> {
        self.reserve([root])?;
        let count = self
            .scopes
            .lock()
            .map_err(|_| {
                ProjectionProtocolError::InvalidBatch(
                    "projection query-scope budget is unavailable".into(),
                )
            })?
            .len();
        Ok(MAX_PROJECTION_QUERY_BATCH_ROWS - count + 1)
    }
}

pub(super) trait ProjectionSnapshotReader: Send + Sync {
    fn snapshot<'a>(
        &'a self,
        request: &'a ProjectionQuerySnapshotRequest,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<ProjectionQuerySnapshot, ProjectionProtocolError>>
                + Send
                + 'a,
        >,
    >;

    fn execution_snapshots<'a>(
        &'a self,
        request: &'a ProjectionExecutionSnapshotBatchRequest,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<ProjectionExecutionSnapshotBatch, ProjectionProtocolError>>
                + Send
                + 'a,
        >,
    >;

    fn graph_snapshot<'a>(
        &'a self,
        request: &'a ProjectionGraphSnapshotRequest,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<ProjectionGraphSnapshot, ProjectionProtocolError>>
                + Send
                + 'a,
        >,
    >;
}

impl<S> ProjectionSnapshotReader for S
where
    S: ProjectionProtocolStore,
{
    fn snapshot<'a>(
        &'a self,
        request: &'a ProjectionQuerySnapshotRequest,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<ProjectionQuerySnapshot, ProjectionProtocolError>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(self.projection_query_snapshot(request))
    }

    fn execution_snapshots<'a>(
        &'a self,
        request: &'a ProjectionExecutionSnapshotBatchRequest,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<ProjectionExecutionSnapshotBatch, ProjectionProtocolError>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(self.projection_execution_snapshot_batch(request))
    }

    fn graph_snapshot<'a>(
        &'a self,
        request: &'a ProjectionGraphSnapshotRequest,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<ProjectionGraphSnapshot, ProjectionProtocolError>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(self.projection_graph_snapshot(request))
    }
}

/// One typed row loaded with the exact protocol revision that fenced the same
/// adapter snapshot.
#[derive(Clone, Debug)]
pub struct LoadedProjection<M> {
    pub model: M,
    pub revision: RecordRevision,
}

/// Framework-owned staging context for one ordered projector input.
///
/// The context intentionally exposes no arbitrary message metadata, dependency
/// value, repository, commit method, or cursor constructor. Handler-visible
/// semantics are limited to the typed payload plus the stable identity helpers
/// below, all of which are included in the canonical input fingerprint.
pub struct CausalProjectorContext {
    event_name: String,
    message_id: String,
    causation_id: String,
    snapshots: Arc<dyn ProjectionSnapshotReader>,
    workspace: Arc<Mutex<Option<ProjectionWorkspace>>>,
    query_scopes: Arc<ProjectionQueryScopeBudget>,
}

impl CausalProjectorContext {
    pub(super) fn new<S>(
        message: &Message,
        snapshots: S,
        workspace: ProjectionWorkspace,
    ) -> (Self, Arc<Mutex<Option<ProjectionWorkspace>>>)
    where
        S: ProjectionSnapshotReader + 'static,
    {
        let workspace = Arc::new(Mutex::new(Some(workspace)));
        (
            Self {
                event_name: message.name().to_string(),
                message_id: message
                    .id()
                    .expect("causal projector contexts require a stable message ID")
                    .to_string(),
                causation_id: message
                    .causation_id()
                    .expect("causal projector contexts require a causation ID")
                    .to_string(),
                snapshots: Arc::new(snapshots),
                workspace: Arc::clone(&workspace),
                query_scopes: Arc::new(ProjectionQueryScopeBudget::default()),
            },
            workspace,
        )
    }

    pub fn event_name(&self) -> &str {
        &self.event_name
    }

    pub fn message_id(&self) -> &str {
        &self.message_id
    }

    pub fn causation_id(&self) -> &str {
        &self.causation_id
    }

    /// Start a stateful read-model graph workspace with no commit capability.
    pub fn read_models(&self) -> ProjectionReadModelWorkspace {
        ProjectionReadModelWorkspace::new(Arc::clone(&self.snapshots), Arc::clone(&self.workspace))
            .with_query_scope_budget(Arc::clone(&self.query_scopes))
    }

    /// Apply a stateful graph diff through the framework-owned causal
    /// workspace.
    ///
    /// This stages only. The projector runtime seals and commits the complete
    /// inbox/revision/checkpoint/observation batch after the handler returns.
    pub async fn apply(
        &self,
        read_models: ProjectionReadModelWorkspace,
    ) -> Result<&Self, HandlerError> {
        if !read_models.belongs_to(&self.workspace) {
            return Err(ProjectionProtocolError::InvalidBatch(
                "projection read-model workspace belongs to a different causal context".into(),
            )
            .into());
        }
        let (plan, cached) = read_models.into_execution_parts()?;
        let prepared = {
            let workspace = self
                .workspace
                .lock()
                .map_err(|_| unavailable_workspace("prepare graph projection"))?;
            let workspace = workspace
                .as_ref()
                .ok_or_else(|| unavailable_workspace("prepare graph projection"))?;
            crate::projection::executor::prepare_graph_projection(workspace, plan, cached)?
        };
        let snapshots = if prepared.needs_snapshot_read() {
            self.query_scopes.reserve(
                prepared
                    .snapshot_request()
                    .requests
                    .iter()
                    .map(|request| &request.scope),
            )?;
            self.snapshots
                .execution_snapshots(prepared.snapshot_request())
                .await?
        } else {
            ProjectionExecutionSnapshotBatch::default()
        };
        prepared.stage(
            self.workspace
                .lock()
                .map_err(|_| unavailable_workspace("apply graph projection"))?
                .as_mut()
                .ok_or_else(|| unavailable_workspace("apply graph projection"))?,
            snapshots,
        )?;
        Ok(self)
    }

    pub(crate) async fn apply_portable(
        &self,
        plan: crate::projection::lower::LoweredProjectionPlan,
    ) -> Result<&Self, HandlerError> {
        let prepared = {
            let workspace = self
                .workspace
                .lock()
                .map_err(|_| unavailable_workspace("prepare portable projection"))?;
            let workspace = workspace
                .as_ref()
                .ok_or_else(|| unavailable_workspace("prepare portable projection"))?;
            crate::projection::executor::prepare_portable_projection(workspace, plan)?
        };
        let snapshots = if prepared.needs_snapshot_read() {
            self.query_scopes.reserve(
                prepared
                    .snapshot_request()
                    .requests
                    .iter()
                    .map(|request| &request.scope),
            )?;
            self.snapshots
                .execution_snapshots(prepared.snapshot_request())
                .await?
        } else {
            ProjectionExecutionSnapshotBatch::default()
        };
        prepared.stage(
            self.workspace
                .lock()
                .map_err(|_| unavailable_workspace("apply portable projection"))?
                .as_mut()
                .ok_or_else(|| unavailable_workspace("apply portable projection"))?,
            snapshots,
        )?;
        Ok(self)
    }

    /// Load one live row and its exact revision from one adapter snapshot.
    ///
    /// Missing rows return `Ok(None)`. A durable tombstone fails closed; use a
    /// separately loaded tombstone revision with [`recreate`](Self::recreate)
    /// only in an explicit recovery/migration projector.
    pub async fn load<M>(&self, key: RowKey) -> Result<Option<LoadedProjection<M>>, HandlerError>
    where
        M: RelationalReadModel,
    {
        let request = self
            .workspace
            .lock()
            .map_err(|_| unavailable_workspace("build projection load"))?
            .as_ref()
            .ok_or_else(|| unavailable_workspace("build projection load"))?
            .query_snapshot_request::<M>(key)?;
        self.query_scopes.reserve([&request.scope])?;
        let snapshot = self.snapshots.snapshot(&request).await?;
        validate_query_snapshot(&request, &snapshot)?;
        match (snapshot.row, snapshot.record) {
            (None, None) => Ok(None),
            (Some(row), Some(record)) if !record.tombstone => Ok(Some(LoadedProjection {
                model: M::from_row(row)?,
                revision: record.revision,
            })),
            (None, Some(record)) if record.tombstone => {
                Err(ProjectionProtocolError::RecordTombstoned {
                    model: M::schema().model_name.clone(),
                }
                .into())
            }
            _ => Err(ProjectionProtocolError::InvalidBatch(format!(
                "projection snapshot for model `{}` returned inconsistent physical/protocol state",
                M::schema().model_name
            ))
            .into()),
        }
    }

    /// Load the exact revision of a durable tombstone, if one exists.
    ///
    /// This is the explicit companion to [`recreate`](Self::recreate). Live
    /// records return `Ok(None)`: callers cannot accidentally use a live
    /// revision to cross the tombstone boundary.
    pub async fn tombstone_revision<M>(
        &self,
        key: RowKey,
    ) -> Result<Option<RecordRevision>, HandlerError>
    where
        M: RelationalReadModel,
    {
        let request = self
            .workspace
            .lock()
            .map_err(|_| unavailable_workspace("build projection tombstone load"))?
            .as_ref()
            .ok_or_else(|| unavailable_workspace("build projection tombstone load"))?
            .query_snapshot_request::<M>(key)?;
        self.query_scopes.reserve([&request.scope])?;
        let snapshot = self.snapshots.snapshot(&request).await?;
        validate_query_snapshot(&request, &snapshot)?;
        match (snapshot.row, snapshot.record) {
            (None, None) => Ok(None),
            (Some(_), Some(record)) if !record.tombstone => Ok(None),
            (None, Some(record)) if record.tombstone => Ok(Some(record.revision)),
            _ => Err(ProjectionProtocolError::InvalidBatch(format!(
                "projection snapshot for model `{}` returned inconsistent physical/protocol state",
                M::schema().model_name
            ))
            .into()),
        }
    }

    /// The polished full-row path: create a missing record or save a live one
    /// under the exact revision read from the same adapter snapshot.
    ///
    /// It never crosses a tombstone. Concurrent writers are caught again by the
    /// adapter's atomic revision fence at commit.
    pub async fn project<M>(&self, model: &M) -> Result<&Self, HandlerError>
    where
        M: RelationalReadModel,
    {
        let key = model.primary_key()?;
        let request = self
            .workspace
            .lock()
            .map_err(|_| unavailable_workspace("build projection upsert"))?
            .as_ref()
            .ok_or_else(|| unavailable_workspace("build projection upsert"))?
            .query_snapshot_request::<M>(key)?;
        self.query_scopes.reserve([&request.scope])?;
        let snapshot = self.snapshots.snapshot(&request).await?;
        validate_query_snapshot(&request, &snapshot)?;
        let mut workspace = self
            .workspace
            .lock()
            .map_err(|_| unavailable_workspace("stage projection upsert"))?;
        let workspace = workspace
            .as_mut()
            .ok_or_else(|| unavailable_workspace("stage projection upsert"))?;
        match (snapshot.row, snapshot.record) {
            (None, None) => {
                workspace.create(model)?;
            }
            (Some(_), Some(record)) if !record.tombstone => {
                workspace.save(model, &record.revision)?;
            }
            (None, Some(record)) if record.tombstone => {
                return Err(ProjectionProtocolError::RecordTombstoned {
                    model: M::schema().model_name.clone(),
                }
                .into());
            }
            _ => {
                return Err(ProjectionProtocolError::InvalidBatch(format!(
                "projection snapshot for model `{}` returned inconsistent physical/protocol state",
                M::schema().model_name
            ))
                .into())
            }
        }
        Ok(self)
    }

    pub fn create<M>(&self, model: &M) -> Result<&Self, HandlerError>
    where
        M: RelationalReadModel,
    {
        self.workspace
            .lock()
            .map_err(|_| unavailable_workspace("stage projection create"))?
            .as_mut()
            .ok_or_else(|| unavailable_workspace("stage projection create"))?
            .create(model)?;
        Ok(self)
    }

    pub fn save<M>(&self, model: &M, expected: &RecordRevision) -> Result<&Self, HandlerError>
    where
        M: RelationalReadModel,
    {
        self.workspace
            .lock()
            .map_err(|_| unavailable_workspace("stage projection save"))?
            .as_mut()
            .ok_or_else(|| unavailable_workspace("stage projection save"))?
            .save(model, expected)?;
        Ok(self)
    }

    pub fn patch<M>(
        &self,
        key: RowKey,
        patch: RowPatch,
        expected: &RecordRevision,
    ) -> Result<&Self, HandlerError>
    where
        M: RelationalReadModel,
    {
        self.workspace
            .lock()
            .map_err(|_| unavailable_workspace("stage projection patch"))?
            .as_mut()
            .ok_or_else(|| unavailable_workspace("stage projection patch"))?
            .patch::<M>(key, patch, expected)?;
        Ok(self)
    }

    pub fn delete<M>(&self, key: RowKey, expected: &RecordRevision) -> Result<&Self, HandlerError>
    where
        M: RelationalReadModel,
    {
        self.workspace
            .lock()
            .map_err(|_| unavailable_workspace("stage projection delete"))?
            .as_mut()
            .ok_or_else(|| unavailable_workspace("stage projection delete"))?
            .delete::<M>(key, expected)?;
        Ok(self)
    }

    pub fn recreate<M>(
        &self,
        model: &M,
        expected_tombstone: &RecordRevision,
    ) -> Result<&Self, HandlerError>
    where
        M: RelationalReadModel,
    {
        self.workspace
            .lock()
            .map_err(|_| unavailable_workspace("stage projection recreate"))?
            .as_mut()
            .ok_or_else(|| unavailable_workspace("stage projection recreate"))?
            .recreate(model, expected_tombstone)?;
        Ok(self)
    }
}

fn validate_query_snapshot(
    request: &ProjectionQuerySnapshotRequest,
    snapshot: &ProjectionQuerySnapshot,
) -> Result<(), HandlerError> {
    let scoped = crate::projection_protocol::ProjectionScopedRowSnapshot {
        scope: request.scope.clone(),
        row: snapshot.row.clone(),
        record: snapshot.record.clone(),
    };
    crate::projection::executor::validate_snapshot_scope(&scoped)?;
    if let Some(row) = &snapshot.row {
        if key_from_row(&request.schema, row)? != request.key {
            return Err(ProjectionProtocolError::ScopeMismatch {
                field: "projection query snapshot row key",
            }
            .into());
        }
    }
    Ok(())
}

pub(super) fn unavailable_workspace(operation: &'static str) -> HandlerError {
    ProjectionProtocolError::InvalidBatch(format!(
        "causal projector workspace is unavailable while attempting to {operation}"
    ))
    .into()
}
