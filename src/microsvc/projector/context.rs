use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use crate::bus::Message;
use crate::projection_protocol::{
    ProjectionProtocolError, ProjectionProtocolStore, ProjectionQuerySnapshot,
    ProjectionQuerySnapshotRequest, ProjectionWorkspace, RecordRevision,
};
use crate::read_model::RelationalReadModel;
use crate::table::{RowKey, RowPatch};

use super::super::HandlerError;

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
        let snapshot = self.snapshots.snapshot(&request).await?;
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
        let snapshot = self.snapshots.snapshot(&request).await?;
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
        let snapshot = self.snapshots.snapshot(&request).await?;
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

pub(super) fn unavailable_workspace(operation: &'static str) -> HandlerError {
    ProjectionProtocolError::InvalidBatch(format!(
        "causal projector workspace is unavailable while attempting to {operation}"
    ))
    .into()
}
