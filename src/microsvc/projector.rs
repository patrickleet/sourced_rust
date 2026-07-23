//! Typed, capability-restricted causal projector routes.
//!
//! A projector handler receives typed input and a [`CausalProjectorContext`].
//! It can load/stage projected rows, but it never receives the dependency
//! bundle, repository, trusted transport cursor, or commit/failure methods.
//! The framework authenticates ordering through the receive adapter, bootstraps
//! the complete compiled topology, and owns the atomic protocol commit.

use std::fmt;
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use serde_json::Value;
use sha2::{Digest, Sha256};

use super::dependencies::CausalProjectionRouteDependencies;
use super::service::{HandlerSpec, Routes};
use super::HandlerError;
use crate::bus::{Message, MessageKind, OrderedDelivery};
use crate::graphql::SurfaceProjector;
use crate::projection_protocol::{
    CompiledProjectionTopology, ProjectionEpoch, ProjectionFailureBatch, ProjectionGeneration,
    ProjectionInputCursor, ProjectionInputDisposition, ProjectionInputFingerprint,
    ProjectionModelOwnership, ProjectionPartition, ProjectionPartitionRuntimeState,
    ProjectionProtocolError, ProjectionProtocolStore, ProjectionQuerySnapshot,
    ProjectionQuerySnapshotRequest, ProjectionWorkspace, ProjectorTopologyId, RecordRevision,
    TrustedProjectionInput,
};
use crate::read_model::RelationalReadModel;
use crate::table::{RowKey, RowPatch, TableSchema};

type ProjectorHandlerFuture =
    Pin<Box<dyn Future<Output = Result<(), HandlerError>> + Send + 'static>>;
type ProjectorHandlerFn<I> =
    dyn Fn(CausalProjectorContext, I) -> ProjectorHandlerFuture + Send + Sync;

const PROJECTION_REPAIR_HANDLE_VERSION: u8 = 1;
const PROJECTION_REPAIR_HANDLE_PREFIX: &str = "distributed-repair-v1:";
const DETERMINISTIC_FAILURE_ID_BYTES: usize = 64;

/// Transferable, non-sensitive operator handle for one durable projection
/// failure.
///
/// The handle deliberately contains only a format version and the globally
/// unique deterministic failure ID. It never serializes a canonical projection
/// partition, which may contain tenant identifiers. During repair, the owning
/// store resolves and validates the exact durable topology/partition scope and
/// the [`Service`](super::Service) verifies that topology is registered.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ProjectionRepairHandle {
    version: u8,
    failure_id: String,
}

impl ProjectionRepairHandle {
    fn for_failure(failure_id: String) -> Self {
        debug_assert!(valid_deterministic_failure_id(&failure_id));
        Self {
            version: PROJECTION_REPAIR_HANDLE_VERSION,
            failure_id,
        }
    }

    pub fn version(&self) -> u8 {
        self.version
    }

    pub fn failure_id(&self) -> &str {
        &self.failure_id
    }
}

impl fmt::Debug for ProjectionRepairHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProjectionRepairHandle")
            .field("version", &self.version)
            .field("failure_id", &self.failure_id)
            .finish()
    }
}

impl fmt::Display for ProjectionRepairHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{PROJECTION_REPAIR_HANDLE_PREFIX}{}",
            self.failure_id
        )
    }
}

/// Failure parsing an operator-supplied [`ProjectionRepairHandle`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProjectionRepairHandleParseError;

impl fmt::Display for ProjectionRepairHandleParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("invalid projection repair handle")
    }
}

impl std::error::Error for ProjectionRepairHandleParseError {}

impl FromStr for ProjectionRepairHandle {
    type Err = ProjectionRepairHandleParseError;

    fn from_str(token: &str) -> Result<Self, Self::Err> {
        let failure_id = token
            .strip_prefix(PROJECTION_REPAIR_HANDLE_PREFIX)
            .filter(|failure_id| valid_deterministic_failure_id(failure_id))
            .ok_or(ProjectionRepairHandleParseError)?;
        Ok(Self {
            version: PROJECTION_REPAIR_HANDLE_VERSION,
            failure_id: failure_id.to_string(),
        })
    }
}

impl serde::Serialize for ProjectionRepairHandle {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> serde::Deserialize<'de> for ProjectionRepairHandle {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let token = String::deserialize(deserializer)?;
        token.parse().map_err(serde::de::Error::custom)
    }
}

fn valid_deterministic_failure_id(failure_id: &str) -> bool {
    failure_id.len() == DETERMINISTIC_FAILURE_ID_BYTES
        && failure_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

trait ProjectionSnapshotReader: Send + Sync {
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
    fn new<S>(
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

fn unavailable_workspace(operation: &'static str) -> HandlerError {
    ProjectionProtocolError::InvalidBatch(format!(
        "causal projector workspace is unavailable while attempting to {operation}"
    ))
    .into()
}

fn boxed_projector_handler<I, F, Fut>(handler: F) -> Arc<ProjectorHandlerFn<I>>
where
    I: Send + 'static,
    F: Fn(CausalProjectorContext, I) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<(), HandlerError>> + Send + 'static,
{
    Arc::new(move |context, input| Box::pin(handler(context, input)))
}

/// Builder for one typed causal projector route.
pub struct CausalProjectorRouteBuilder<D, I> {
    routes: Routes<D>,
    declaration: SurfaceProjector,
    schemas: Vec<&'static TableSchema>,
    _input: PhantomData<fn(I)>,
}

impl<D, I> CausalProjectorRouteBuilder<D, I>
where
    D: CausalProjectionRouteDependencies + Send + Sync + 'static,
    I: serde::de::DeserializeOwned + Send + 'static,
{
    pub(super) fn new(routes: Routes<D>, declaration: SurfaceProjector) -> Self {
        Self {
            routes,
            declaration,
            schemas: Vec::new(),
            _input: PhantomData,
        }
    }

    /// Register one complete typed output model. Call once for every model in
    /// the reused [`SurfaceProjector`] declaration.
    pub fn model<M>(mut self) -> Self
    where
        M: RelationalReadModel + 'static,
    {
        self.schemas.push(M::schema());
        self
    }

    /// Register the capability-restricted handler.
    ///
    /// Invalid/incomplete topology declarations panic at service construction,
    /// before transport traffic can start, matching ordinary duplicate-route
    /// registration behavior.
    pub fn handle<F, Fut>(self, handler: F) -> Routes<D>
    where
        F: Fn(CausalProjectorContext, I) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), HandlerError>> + Send + 'static,
    {
        let change_epoch = self.declaration.change_epoch.as_ref().unwrap_or_else(|| {
            panic!(
                "causal projector `{}` requires a change-log epoch",
                self.declaration.name
            )
        });
        let change_epoch = ProjectionEpoch::new(change_epoch.clone()).unwrap_or_else(|error| {
            panic!(
                "causal projector `{}` has an invalid change-log epoch: {error}",
                self.declaration.name
            )
        });
        let compiled = CompiledProjectionTopology::compile(
            &self.declaration.name,
            &self.declaration.facts,
            &self.declaration.models,
            &self.declaration.partition,
            self.schemas,
        )
        .unwrap_or_else(|error| {
            panic!(
                "causal projector `{}` has an invalid compiled topology: {error}",
                self.declaration.name
            )
        });
        let spec = HandlerSpec::projector(self.declaration.facts.clone());
        self.routes.register_projector(
            spec,
            Box::new(RegisteredProjector::<I> {
                compiled,
                change_epoch,
                handle: boxed_projector_handler(handler),
            }),
        )
    }
}

pub(super) type ProjectorDispatchFuture<'a> =
    Pin<Box<dyn Future<Output = Result<(), HandlerError>> + Send + 'a>>;
pub(super) type ProjectorRepairFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Option<ProjectionGeneration>, HandlerError>> + Send + 'a>>;
pub(super) type ProjectorRepairLookupFuture<'a> =
    Pin<Box<dyn Future<Output = Result<bool, HandlerError>> + Send + 'a>>;

pub(super) trait ErasedProjectorHandler<D>: Send + Sync {
    fn dispatch<'a>(
        &'a self,
        dependencies: &'a D,
        message: &'a Message,
        ordered: Option<&'a OrderedDelivery>,
    ) -> ProjectorDispatchFuture<'a>;

    fn registration(&self) -> ProjectorRegistration;

    fn bootstrap<'a>(&'a self, dependencies: &'a D) -> ProjectorDispatchFuture<'a>;

    fn repair<'a>(
        &'a self,
        dependencies: &'a D,
        handle: &'a ProjectionRepairHandle,
    ) -> ProjectorRepairFuture<'a>;

    fn locates_failure<'a>(
        &'a self,
        dependencies: &'a D,
        handle: &'a ProjectionRepairHandle,
    ) -> ProjectorRepairLookupFuture<'a>;
}

#[derive(Clone)]
pub(super) struct ProjectorRegistration {
    pub(super) topology: ProjectorTopologyId,
    pub(super) ownership: Vec<ProjectionModelOwnership>,
}

struct RegisteredProjector<I> {
    compiled: CompiledProjectionTopology,
    change_epoch: ProjectionEpoch,
    handle: Arc<ProjectorHandlerFn<I>>,
}

impl<D, I> ErasedProjectorHandler<D> for RegisteredProjector<I>
where
    D: CausalProjectionRouteDependencies + Send + Sync + 'static,
    I: serde::de::DeserializeOwned + Send + 'static,
{
    fn registration(&self) -> ProjectorRegistration {
        ProjectorRegistration {
            topology: self.compiled.topology().clone(),
            ownership: self.compiled.ownership().to_vec(),
        }
    }

    fn bootstrap<'a>(&'a self, dependencies: &'a D) -> ProjectorDispatchFuture<'a> {
        Box::pin(async move {
            dependencies
                .__causal_projection_store()
                .register_projection_models(self.compiled.topology(), self.compiled.ownership())
                .await?;
            Ok(())
        })
    }

    fn repair<'a>(
        &'a self,
        dependencies: &'a D,
        handle: &'a ProjectionRepairHandle,
    ) -> ProjectorRepairFuture<'a> {
        Box::pin(async move {
            let store = dependencies.__causal_projection_store();
            let Some(location) = store
                .projection_failure_location(handle.failure_id())
                .await?
            else {
                return Ok(None);
            };
            if &location.topology != self.compiled.topology() {
                return Ok(None);
            }
            store
                .register_projection_models(self.compiled.topology(), self.compiled.ownership())
                .await?;
            let generation = store
                .repair_projection(&location.topology, &location.partition, handle.failure_id())
                .await?;
            Ok(Some(generation))
        })
    }

    fn locates_failure<'a>(
        &'a self,
        dependencies: &'a D,
        handle: &'a ProjectionRepairHandle,
    ) -> ProjectorRepairLookupFuture<'a> {
        Box::pin(async move {
            let location = dependencies
                .__causal_projection_store()
                .projection_failure_location(handle.failure_id())
                .await?;
            Ok(location
                .as_ref()
                .is_some_and(|location| &location.topology == self.compiled.topology()))
        })
    }

    fn dispatch<'a>(
        &'a self,
        dependencies: &'a D,
        message: &'a Message,
        ordered: Option<&'a OrderedDelivery>,
    ) -> ProjectorDispatchFuture<'a> {
        Box::pin(async move {
            let ordered = ordered.ok_or_else(|| {
                HandlerError::UnqualifiedProjectionDelivery(format!(
                    "causal projector `{}` requires adapter-authenticated ordered delivery",
                    self.compiled.topology().name()
                ))
            })?;
            if message.kind != MessageKind::Event {
                return Err(HandlerError::UnqualifiedProjectionDelivery(
                    "causal projector routes accept only event deliveries".into(),
                ));
            }
            let message_id = message.id().ok_or_else(|| {
                HandlerError::UnqualifiedProjectionDelivery(
                    "causal projector delivery is missing a stable message ID".into(),
                )
            })?;
            let causation_id = message.causation_id().ok_or_else(|| {
                HandlerError::UnqualifiedProjectionDelivery(
                    "causal projector delivery is missing a causation ID".into(),
                )
            })?;

            let mut canonical_input = None;
            let partition_value = if self.compiled.partition().requires_input() {
                match message.payload_json::<Value>() {
                    Ok(input) => match self.compiled.partition().resolve(&input) {
                        Ok(partition) => {
                            canonical_input = Some(input);
                            partition
                        }
                        Err(error) => {
                            return record_ingress_failure(
                                dependencies.__causal_projection_store(),
                                &self.compiled,
                                self.change_epoch.clone(),
                                message,
                                ordered,
                                message_id,
                                causation_id,
                                error.into(),
                            )
                            .await
                        }
                    },
                    Err(error) => {
                        return record_ingress_failure(
                            dependencies.__causal_projection_store(),
                            &self.compiled,
                            self.change_epoch.clone(),
                            message,
                            ordered,
                            message_id,
                            causation_id,
                            HandlerError::from(error),
                        )
                        .await
                    }
                }
            } else {
                self.compiled.partition().resolve(&Value::Null)?
            };
            let projection_partition = self
                .compiled
                .codec()
                .encode_partition(partition_value.as_ref())
                .map_err(|error| {
                    ProjectionProtocolError::InvalidBatch(format!(
                        "causal projector partition could not be encoded: {error}"
                    ))
                })?;
            let cursor = ProjectionInputCursor::new(
                self.compiled.topology().clone(),
                projection_partition.clone(),
                ordered.source().clone(),
                ordered.epoch().clone(),
                ordered.position(),
            )
            .map_err(ProjectionProtocolError::from)?;
            let fingerprint = canonical_message_fingerprint(message);
            // Adapter gap-free evidence is scoped to its source partition.
            // A dynamic input-path partition splits that sequence, so it can
            // preserve ordering but cannot prove gap-free progress for any one
            // derived projection partition.
            let gap_free =
                ordered.is_gap_free() && self.compiled.partition().preserves_source_sequence();
            let store = dependencies.__causal_projection_store();

            // Idempotent global ownership bootstrap deliberately precedes every
            // partition read/handler invocation. Adapters make repeated exact
            // registration cheap and reject any conflicting owner.
            store
                .register_projection_models(self.compiled.topology(), self.compiled.ownership())
                .await?;
            let runtime_state = store
                .projection_partition_runtime_state(self.compiled.topology(), &projection_partition)
                .await?;
            let generation = preflight_runtime_state(
                runtime_state.as_ref(),
                &cursor,
                fingerprint,
                message_id,
                causation_id,
                gap_free,
            )?;
            let trusted = TrustedProjectionInput::mint(
                cursor,
                fingerprint,
                message_id,
                causation_id,
                generation,
                gap_free,
            )?;
            match store.projection_input_disposition(&trusted).await? {
                ProjectionInputDisposition::Pending => {}
                ProjectionInputDisposition::Duplicate(_) | ProjectionInputDisposition::Stale(_) => {
                    return Ok(())
                }
            }
            let failure_input = trusted.clone();
            let canonical_input = match canonical_input {
                Some(input) => input,
                None => match message.payload_json::<Value>() {
                    Ok(input) => input,
                    Err(error) => {
                        return handle_projector_error(
                            store,
                            failure_input,
                            self.change_epoch.clone(),
                            "json_decode",
                            HandlerError::from(error),
                        )
                        .await
                    }
                },
            };
            let input = match serde_json::from_value::<I>(canonical_input) {
                Ok(input) => input,
                Err(error) => {
                    return handle_projector_error(
                        store,
                        failure_input,
                        self.change_epoch.clone(),
                        "typed_decode",
                        HandlerError::from(error),
                    )
                    .await
                }
            };
            let workspace = ProjectionWorkspace::new(
                self.compiled.codec(),
                partition_value,
                trusted,
                self.change_epoch.clone(),
            )?;
            let (context, workspace) =
                CausalProjectorContext::new(message, D::Store::clone(store), workspace);
            if let Err(error) = (self.handle)(context, input).await {
                return handle_projector_error(
                    store,
                    failure_input,
                    self.change_epoch.clone(),
                    "handler_error",
                    error,
                )
                .await;
            }
            let workspace = workspace
                .lock()
                .map_err(|_| unavailable_workspace("seal projection batch"))?
                .take()
                .ok_or_else(|| unavailable_workspace("seal projection batch"))?;
            let batch = match workspace.into_batch() {
                Ok(batch) => batch,
                Err(error) => {
                    return record_terminal_protocol_failure(
                        store,
                        failure_input,
                        self.change_epoch.clone(),
                        "workspace_seal",
                        error,
                    )
                    .await
                }
            };
            match store.commit_projection(batch).await {
                Ok(_) => Ok(()),
                Err(ProjectionProtocolError::PartitionStopped { failure_id }) => {
                    Err(terminal_recorded(failure_id))
                }
                Err(error) if projection_error_is_retryable(&error) => Err(error.into()),
                Err(error) => {
                    record_terminal_protocol_failure(
                        store,
                        failure_input,
                        self.change_epoch.clone(),
                        "commit_projection",
                        error,
                    )
                    .await
                }
            }
        })
    }
}

async fn record_ingress_failure<S>(
    store: &S,
    compiled: &CompiledProjectionTopology,
    change_epoch: ProjectionEpoch,
    message: &Message,
    ordered: &OrderedDelivery,
    message_id: &str,
    causation_id: &str,
    error: HandlerError,
) -> Result<(), HandlerError>
where
    S: ProjectionProtocolStore,
{
    let partition =
        ProjectionPartition::new(b"distributed.projection.ingress-failure-partition.v1\0".to_vec())
            .map_err(ProjectionProtocolError::from)?;
    let cursor = ProjectionInputCursor::new(
        compiled.topology().clone(),
        partition.clone(),
        ordered.source().clone(),
        ordered.epoch().clone(),
        ordered.position(),
    )
    .map_err(ProjectionProtocolError::from)?;
    let fingerprint = canonical_message_fingerprint(message);
    store
        .register_projection_models(compiled.topology(), compiled.ownership())
        .await?;
    let runtime_state = store
        .projection_partition_runtime_state(compiled.topology(), &partition)
        .await?;
    let generation = preflight_runtime_state(
        runtime_state.as_ref(),
        &cursor,
        fingerprint,
        message_id,
        causation_id,
        false,
    )?;
    let input = TrustedProjectionInput::mint(
        cursor,
        fingerprint,
        message_id,
        causation_id,
        generation,
        false,
    )?;
    handle_projector_error(store, input, change_epoch, "ingress_partition", error).await
}

fn preflight_runtime_state(
    state: Option<&ProjectionPartitionRuntimeState>,
    cursor: &ProjectionInputCursor,
    fingerprint: ProjectionInputFingerprint,
    message_id: &str,
    causation_id: &str,
    gap_free: bool,
) -> Result<ProjectionGeneration, HandlerError> {
    let Some(state) = state else {
        return Ok(ProjectionGeneration::initial());
    };
    if let Some(failure_id) = &state.stopped_failure_id {
        return Err(terminal_recorded(failure_id.clone()));
    }
    if let Some(pending) = &state.pending_retry {
        if &pending.input != cursor {
            return Err(terminal_recorded(pending.failure_id.clone()));
        }
        if pending.input_fingerprint != fingerprint
            || pending.message_id != message_id
            || pending.causation_id != causation_id
            || pending.gap_free != gap_free
        {
            return Err(terminal_recorded(pending.failure_id.clone()));
        }
    }
    Ok(state.active_generation)
}

async fn handle_projector_error<S>(
    store: &S,
    input: TrustedProjectionInput,
    change_epoch: ProjectionEpoch,
    code: &'static str,
    error: HandlerError,
) -> Result<(), HandlerError>
where
    S: ProjectionProtocolStore,
{
    if error.is_projection_retryable() {
        return Err(error);
    }
    let detail = error.to_string();
    let failure_id =
        record_terminal_failure(store, input, change_epoch, code, detail.as_bytes()).await?;
    Err(terminal_recorded(failure_id))
}

async fn record_terminal_protocol_failure<S>(
    store: &S,
    input: TrustedProjectionInput,
    change_epoch: ProjectionEpoch,
    code: &'static str,
    error: ProjectionProtocolError,
) -> Result<(), HandlerError>
where
    S: ProjectionProtocolStore,
{
    if let ProjectionProtocolError::PartitionStopped { failure_id } = error {
        return Err(terminal_recorded(failure_id));
    }
    if projection_error_is_retryable(&error) {
        return Err(error.into());
    }
    let detail = error.to_string();
    let failure_id =
        record_terminal_failure(store, input, change_epoch, code, detail.as_bytes()).await?;
    Err(terminal_recorded(failure_id))
}

async fn record_terminal_failure<S>(
    store: &S,
    input: TrustedProjectionInput,
    change_epoch: ProjectionEpoch,
    code: &'static str,
    detail: &[u8],
) -> Result<String, HandlerError>
where
    S: ProjectionProtocolStore,
{
    const MAX_FAILURE_DETAIL_BYTES: usize = 1024 * 1024;

    let failure_id = deterministic_failure_id(&input);
    let detail = if detail.len() > MAX_FAILURE_DETAIL_BYTES {
        &detail[..MAX_FAILURE_DETAIL_BYTES]
    } else {
        detail
    };
    let batch = ProjectionFailureBatch::new(
        input,
        change_epoch,
        failure_id.clone(),
        code,
        detail.to_vec(),
    )?;
    match store.record_projection_failure(batch).await {
        Ok(_) => Ok(failure_id),
        // Another worker can durably stop this partition between our preflight
        // and failure write. Retaining the current exact source position and
        // stopping is the same required outcome as observing it in preflight.
        Err(ProjectionProtocolError::PartitionStopped { failure_id }) => {
            Err(terminal_recorded(failure_id))
        }
        Err(error) => Err(error.into()),
    }
}

fn terminal_recorded(failure_id: String) -> HandlerError {
    HandlerError::ProjectionTerminalRecorded {
        repair: ProjectionRepairHandle::for_failure(failure_id),
    }
}

fn deterministic_failure_id(input: &TrustedProjectionInput) -> String {
    let mut digest = Sha256::new();
    digest.update(b"distributed.causal-projector.failure-id.v1\0");
    digest.update(input.cursor.topology().canonical_bytes());
    digest.update(input.cursor.projection_partition().canonical_bytes());
    digest.update(input.cursor.source().name().as_bytes());
    digest.update(input.cursor.source().canonical_partition_bytes());
    digest.update(input.cursor.epoch().as_str().as_bytes());
    digest.update(input.cursor.position().to_be_bytes());
    digest.update(input.generation.get().to_be_bytes());
    format!("{:x}", digest.finalize())
}

fn canonical_message_fingerprint(message: &Message) -> ProjectionInputFingerprint {
    let canonical = serde_json::json!({
        "version": 1,
        "id": message.id(),
        "name": message.name(),
        "kind": message.kind.as_str(),
        "content_type": message.content_type,
        "payload": message.payload,
        "causation_id": message.causation_id(),
    });
    ProjectionInputFingerprint::from_canonical_bytes(
        &serde_json::to_vec(&canonical)
            .expect("a canonical transport message contains only serializable primitives"),
    )
}

pub(super) fn projection_error_is_retryable(error: &ProjectionProtocolError) -> bool {
    use crate::lock::RetryClass;
    use crate::table::TableStoreError;

    match error {
        ProjectionProtocolError::Repository(error) => error.is_retryable(),
        ProjectionProtocolError::Table(TableStoreError::ConcurrencyConflict { .. })
        | ProjectionProtocolError::Table(TableStoreError::NotFound { .. })
        | ProjectionProtocolError::RecordRevisionConflict { .. }
        | ProjectionProtocolError::GenerationFenced { .. } => true,
        ProjectionProtocolError::Table(TableStoreError::BackendStorage { retryable, .. }) => {
            *retryable
        }
        ProjectionProtocolError::Table(TableStoreError::Lock(error)) => {
            error.kind() == RetryClass::Retryable
        }
        ProjectionProtocolError::Validation(_)
        | ProjectionProtocolError::Table(_)
        | ProjectionProtocolError::InvalidBatch(_)
        | ProjectionProtocolError::ScopeMismatch { .. }
        | ProjectionProtocolError::IncomparableInput
        | ProjectionProtocolError::InputCorruption
        | ProjectionProtocolError::MessageIdReuse { .. }
        | ProjectionProtocolError::PartitionStopped { .. }
        | ProjectionProtocolError::RecordMissing { .. }
        | ProjectionProtocolError::RecordAlreadyExists { .. }
        | ProjectionProtocolError::RecordTombstoned { .. }
        | ProjectionProtocolError::RecreateRequiresTombstone { .. }
        | ProjectionProtocolError::CausalWriteRequired { .. }
        | ProjectionProtocolError::PositionOverflow { .. } => false,
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use crate::bus::{Bus, FailurePolicy, InMemoryBus, RunOptions};
    use crate::projection_protocol::{
        ProjectionCheckpointProbe, ProjectionProtocolStore, ProjectionQuerySnapshotRequest,
    };
    use crate::table::{RowKey, RowValue};
    use crate::{InMemoryRepository, RelationalReadModel};

    use super::*;
    use crate::microsvc::Service;

    const FACT_NAME: &str = "task15.todo_changed";

    #[derive(Clone, Debug, serde::Deserialize)]
    struct TodoChanged {
        id: String,
        title: String,
    }

    #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize, crate::ReadModel)]
    #[readmodel(table = "task15_primary_views", primary_key = ["id"])]
    struct PrimaryView {
        id: String,
        title: String,
    }

    #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize, crate::ReadModel)]
    #[readmodel(table = "task15_secondary_views", primary_key = ["id"])]
    struct SecondaryView {
        id: String,
        title: String,
    }

    #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize, crate::ReadModel)]
    #[readmodel(table = "task15_malformed_views", primary_key = ["id"])]
    struct MalformedView {
        id: String,
    }

    fn primary_projector() -> SurfaceProjector {
        SurfaceProjector::new("task15_a_primary")
            .facts([FACT_NAME])
            .models(["PrimaryView"])
            .change_epoch("task15-primary-v1")
    }

    fn secondary_projector() -> SurfaceProjector {
        SurfaceProjector::new("task15_b_secondary")
            .facts([FACT_NAME])
            .models(["SecondaryView"])
            .change_epoch("task15-secondary-v1")
    }

    fn fact_message(id: &str, title: &str) -> Message {
        Message::new(
            FACT_NAME,
            MessageKind::Event,
            serde_json::to_vec(&serde_json::json!({ "id": id, "title": title })).unwrap(),
        )
        .with_id(format!("fact-{id}"))
        .with_metadata(crate::trace_context::CAUSATION_ID, format!("command-{id}"))
    }

    fn row_key(id: &str) -> RowKey {
        RowKey::new([("id", RowValue::String(id.to_string()))])
    }

    #[tokio::test]
    async fn public_builder_fans_one_fact_out_and_replays_partial_success_exactly() {
        let repository = InMemoryRepository::new();
        let bus = InMemoryBus::new();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let fail_secondary_once = Arc::new(AtomicBool::new(true));

        let primary = primary_projector();
        let secondary = secondary_projector();
        let primary_calls = Arc::clone(&calls);
        let secondary_calls = Arc::clone(&calls);
        let secondary_failure = Arc::clone(&fail_secondary_once);
        let routes = Routes::new()
            .with_read_model_store(repository.clone())
            .causal_projector::<TodoChanged>(primary.clone())
            .model::<PrimaryView>()
            .handle(move |context: CausalProjectorContext, fact: TodoChanged| {
                let calls = Arc::clone(&primary_calls);
                async move {
                    {
                        let mut calls = calls.lock().unwrap();
                        if calls.contains(&"primary") {
                            return Err(HandlerError::Rejected(
                                "an applied projector must not be reinvoked on sibling retry"
                                    .into(),
                            ));
                        }
                        calls.push("primary");
                    }
                    context
                        .project(&PrimaryView {
                            id: fact.id,
                            title: fact.title,
                        })
                        .await?;
                    Ok(())
                }
            })
            .causal_projector::<TodoChanged>(secondary)
            .model::<SecondaryView>()
            .handle(move |context: CausalProjectorContext, fact: TodoChanged| {
                let calls = Arc::clone(&secondary_calls);
                let fail = Arc::clone(&secondary_failure);
                async move {
                    calls.lock().unwrap().push("secondary");
                    if fail.swap(false, Ordering::SeqCst) {
                        return Err(HandlerError::Other(
                            "injected transient projector failure".into(),
                        ));
                    }
                    context
                        .project(&SecondaryView {
                            id: fact.id,
                            title: fact.title,
                        })
                        .await?;
                    Ok(())
                }
            });
        let service = Service::new().routes(routes).with_bus(bus.clone());
        bus.publish_message(fact_message("todo-1", "causal cache"))
            .await
            .unwrap();

        service.run(RunOptions::idempotent()).await.unwrap();
        assert_eq!(
            *calls.lock().unwrap(),
            vec!["primary", "secondary", "secondary"],
            "an applied sibling is skipped while only the failed projector retries"
        );

        let compiled = CompiledProjectionTopology::compile(
            &primary.name,
            &primary.facts,
            &primary.models,
            &primary.partition,
            [PrimaryView::schema()],
        )
        .unwrap();
        let codec = compiled.codec();
        let partition = codec.encode_partition(None).unwrap();
        let ordered = bus.ordered_topic_evidence(FACT_NAME, 0);
        let cursor = ProjectionInputCursor::new(
            compiled.topology().clone(),
            partition.clone(),
            ordered.source().clone(),
            ordered.epoch().clone(),
            ordered.position(),
        )
        .unwrap();
        let snapshot = repository
            .projection_query_snapshot(
                &ProjectionQuerySnapshotRequest::new(
                    &codec,
                    None,
                    "PrimaryView",
                    row_key("todo-1"),
                    vec![ProjectionCheckpointProbe::new(
                        compiled.topology().clone(),
                        partition,
                        ordered.source().clone(),
                        ordered.epoch().clone(),
                        ProjectionGeneration::initial(),
                    )],
                )
                .unwrap(),
            )
            .await
            .unwrap();
        let row = snapshot.row.expect("projected physical row");
        assert_eq!(row.get_serde::<String>("title").unwrap(), "causal cache");
        let record = snapshot.record.expect("projection record revision");
        assert_eq!(record.revision.incarnation(), 1);
        assert_eq!(record.revision.revision(), 1);
        let checkpoint = snapshot.checkpoints[0]
            .checkpoint
            .as_ref()
            .expect("source checkpoint");
        assert_eq!(checkpoint.input(), &cursor);
        assert!(checkpoint.is_gap_free());
    }

    #[tokio::test]
    async fn service_fans_same_fact_out_across_projector_only_route_bundles() {
        static INFERRED_PRIMARY_CALLS: AtomicUsize = AtomicUsize::new(0);

        let repository = InMemoryRepository::new();
        let bus = InMemoryBus::new();
        let secondary_calls = Arc::new(AtomicUsize::new(0));
        INFERRED_PRIMARY_CALLS.store(0, Ordering::SeqCst);

        let primary_routes = Routes::new()
            .with_read_model_store(repository.clone())
            .causal_projector::<TodoChanged>(primary_projector())
            .model::<PrimaryView>()
            .handle(|context, fact: TodoChanged| async move {
                INFERRED_PRIMARY_CALLS.fetch_add(1, Ordering::SeqCst);
                context
                    .project(&PrimaryView {
                        id: fact.id,
                        title: fact.title,
                    })
                    .await?;
                Ok(())
            });
        let secondary_seen = Arc::clone(&secondary_calls);
        let secondary_routes = Routes::new()
            .with_read_model_store(repository)
            .causal_projector::<TodoChanged>(secondary_projector())
            .model::<SecondaryView>()
            .handle(move |context: CausalProjectorContext, fact: TodoChanged| {
                let seen = Arc::clone(&secondary_seen);
                async move {
                    seen.fetch_add(1, Ordering::SeqCst);
                    context
                        .project(&SecondaryView {
                            id: fact.id,
                            title: fact.title,
                        })
                        .await?;
                    Ok(())
                }
            });
        let service = Service::new()
            .routes(primary_routes)
            .routes(secondary_routes)
            .with_bus(bus.clone());
        assert_eq!(
            service.subscription_plan().events,
            vec![FACT_NAME.to_string()]
        );
        bus.publish_message(fact_message("todo-2", "cross bundle"))
            .await
            .unwrap();
        service.run(RunOptions::idempotent()).await.unwrap();

        assert_eq!(INFERRED_PRIMARY_CALLS.load(Ordering::SeqCst), 1);
        assert_eq!(secondary_calls.load(Ordering::SeqCst), 1);
    }

    fn malformed_service(repository: InMemoryRepository, invoked: Arc<AtomicUsize>) -> Service {
        let projector = SurfaceProjector::new("task15_malformed")
            .facts(["task15.malformed"])
            .models(["MalformedView"])
            .change_epoch("task15-malformed-v1")
            .partition_by(["tenant"]);
        Service::new().routes(
            Routes::new()
                .with_read_model_store(repository)
                .causal_projector::<TodoChanged>(projector)
                .model::<MalformedView>()
                .handle(
                    move |_context: CausalProjectorContext, _fact: TodoChanged| {
                        let invoked = Arc::clone(&invoked);
                        async move {
                            invoked.fetch_add(1, Ordering::SeqCst);
                            Ok(())
                        }
                    },
                ),
        )
    }

    fn repair_handle(error: &crate::bus::TransportError) -> ProjectionRepairHandle {
        error
            .source()
            .and_then(|source| source.downcast_ref::<HandlerError>())
            .and_then(HandlerError::projection_repair_handle)
            .cloned()
            .expect("terminal transport error carries an operator repair handle")
    }

    #[tokio::test]
    async fn malformed_ingress_emits_safe_handle_and_repair_restart_stays_terminal() {
        let repository = InMemoryRepository::new();
        let bus = InMemoryBus::new();
        let invoked = Arc::new(AtomicUsize::new(0));
        let malformed = Message::new(
            "task15.malformed",
            MessageKind::Event,
            b"{tenant-secret".to_vec(),
        )
        .with_id("malformed-1")
        .with_metadata(crate::trace_context::CAUSATION_ID, "command-malformed");
        bus.publish_message(malformed).await.unwrap();

        let first = malformed_service(repository.clone(), Arc::clone(&invoked))
            .with_bus(bus.clone())
            .run(RunOptions::idempotent())
            .await
            .expect_err("malformed ingress must retain its exact position and stop");
        let first_handle = repair_handle(&first);
        let token = first_handle.to_string();
        assert!(!token.contains("tenant-secret"));
        assert_eq!(
            token.parse::<ProjectionRepairHandle>().unwrap(),
            first_handle
        );
        assert_eq!(
            serde_json::from_str::<ProjectionRepairHandle>(
                &serde_json::to_string(&first_handle).unwrap()
            )
            .unwrap(),
            first_handle
        );

        let repaired = malformed_service(repository, Arc::clone(&invoked));
        assert_eq!(
            repaired
                .repair_projection(&first_handle)
                .await
                .unwrap()
                .get(),
            2
        );
        let second = repaired
            .with_bus(bus)
            .run(RunOptions::idempotent())
            .await
            .expect_err("unchanged malformed bytes must stop the repaired generation again");
        let second_handle = repair_handle(&second);
        assert_ne!(second_handle, first_handle);
        assert_eq!(invoked.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn repair_handle_parser_rejects_noncanonical_or_non_hash_tokens() {
        for token in [
            "",
            "distributed-repair-v2:abcd",
            "distributed-repair-v1:abcd",
            "distributed-repair-v1:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        ] {
            assert!(token.parse::<ProjectionRepairHandle>().is_err(), "{token}");
        }
    }

    #[tokio::test]
    async fn unrecorded_permanent_projector_failure_retains_and_stops_under_drop_policies() {
        for policy in [FailurePolicy::DeadLetter, FailurePolicy::LogAndAck] {
            let repository = InMemoryRepository::new();
            let bus = InMemoryBus::new();
            let invoked = Arc::new(AtomicUsize::new(0));
            let handler_invoked = Arc::clone(&invoked);
            let service = Service::new()
                .routes(
                    Routes::new()
                        .with_read_model_store(repository)
                        .causal_projector::<TodoChanged>(primary_projector())
                        .model::<PrimaryView>()
                        .handle(move |_context, _fact| {
                            let invoked = Arc::clone(&handler_invoked);
                            async move {
                                invoked.fetch_add(1, Ordering::SeqCst);
                                Ok(())
                            }
                        }),
                )
                .with_bus(bus.clone());
            bus.publish_message(Message::new(
                FACT_NAME,
                MessageKind::Event,
                br#"{"id":"secret-tenant","title":"must not cross"}"#.to_vec(),
            ))
            .await
            .unwrap();

            let error = service
                .run(RunOptions::idempotent().with_failure_policy(policy))
                .await
                .expect_err("unrecorded permanent projector failure must stop the runner");
            assert!(error.is_permanent());
            assert!(error.should_retain_and_stop());
            let halted = error
                .source()
                .and_then(|source| source.downcast_ref::<HandlerError>());
            assert!(
                matches!(halted, Some(HandlerError::ProjectionDeliveryHalted { .. })),
                "projector-only dispatch must erase the unrecorded internal failure"
            );
            assert!(
                matches!(
                    halted
                        .and_then(|halted| halted.source())
                        .and_then(|source| source.downcast_ref::<HandlerError>()),
                    Some(HandlerError::UnqualifiedProjectionDelivery(_))
                ),
                "operator diagnostics retain the original failure only as an error source"
            );
            assert!(!error.to_string().contains("secret-tenant"));
            assert_eq!(invoked.load(Ordering::SeqCst), 0);
        }
    }
}
