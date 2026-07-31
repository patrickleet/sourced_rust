//! Commit-less read-model graph workspace for causal projector handlers.
//!
//! Crate-private. Application authoring uses modeled/mutation handlers.
//! APIs here remain for in-crate protocol unit tests; suppress dead_code noise
//! when the production path only uses portable apply.

#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap, HashSet};
use std::marker::PhantomData;
use std::sync::{Arc, Mutex};

use crate::projection::executor::validate_snapshot_scope;
use crate::projection_protocol::{
    ProjectionGraphSnapshot, ProjectionGraphSnapshotRequest, ProjectionProtocolError,
    ProjectionRecordScope, ProjectionScopedRowSnapshot, ProjectionWorkspace, RecordRevision,
    MAX_PROJECTION_QUERY_BATCH_ROWS,
};
use crate::read_model::{
    ReadModelIncludeRows, ReadModelWorkspace, RelationalReadModel, RelationalReadModelIncludes,
    Versioned,
};
use crate::table::{
    column_name_for, key_from_row, RelationshipKind, RowKey, RowPatch, RowValues, TableStoreError,
    TableWritePlan,
};

use super::super::HandlerError;
use super::context::{ProjectionQueryScopeBudget, ProjectionSnapshotReader};

static STORE_FREE_READ_MODELS: () = ();

/// One graph root loaded with its exact causal protocol revision.
#[derive(Clone, Debug)]
pub(crate) struct LoadedProjectionGraph<M> {
    /// Hydrated root data, including every requested relationship.
    pub data: M,
    /// Exact protocol revision from the same adapter snapshot as `data`.
    pub revision: RecordRevision,
}

/// Crate-private graph staging workspace for protocol tests and portable apply.
///
/// Application projectors must use modeled/mutation handlers
/// ([`super::ModeledProjection::apply`]), not a public load/mutate/sync ORM
/// authoring surface.
#[allow(dead_code)] // exercised from `#[cfg(test)]` modules and protocol fixtures
pub(crate) struct ProjectionReadModelWorkspace {
    snapshots: Arc<dyn ProjectionSnapshotReader>,
    causal: Arc<Mutex<Option<ProjectionWorkspace>>>,
    read_models: ReadModelWorkspace<'static, ()>,
    revisions: HashMap<ProjectionRecordScope, ProjectionScopedRowSnapshot>,
    graphs: HashMap<ProjectionRecordScope, ProjectionGraphSnapshot>,
    query_scope_count: usize,
    loaded_record_scopes: HashSet<ProjectionRecordScope>,
    query_scopes: Arc<ProjectionQueryScopeBudget>,
}

#[allow(dead_code)] // crate-private staging surface; exercised by cfg(test) modules
impl ProjectionReadModelWorkspace {
    pub(super) fn new(
        snapshots: Arc<dyn ProjectionSnapshotReader>,
        causal: Arc<Mutex<Option<ProjectionWorkspace>>>,
    ) -> Self {
        Self {
            snapshots,
            causal,
            read_models: ReadModelWorkspace::new(&STORE_FREE_READ_MODELS),
            revisions: HashMap::new(),
            graphs: HashMap::new(),
            query_scope_count: 0,
            loaded_record_scopes: HashSet::new(),
            query_scopes: Arc::new(ProjectionQueryScopeBudget::default()),
        }
    }

    pub(super) fn with_query_scope_budget(
        mut self,
        query_scopes: Arc<ProjectionQueryScopeBudget>,
    ) -> Self {
        self.query_scopes = query_scopes;
        self
    }

    /// Begin one coherent root/include load.
    pub fn load<M>(&mut self, key: RowKey) -> ProjectionGraphLoadBuilder<'_, M>
    where
        M: RelationalReadModel + RelationalReadModelIncludes,
    {
        ProjectionGraphLoadBuilder {
            workspace: self,
            key,
            includes: Vec::new(),
            marker: PhantomData,
        }
    }

    /// Diff a previously loaded graph through the ordinary read-model
    /// workspace semantics.
    pub fn sync<M>(&mut self, model: M) -> Result<&mut Self, TableStoreError>
    where
        M: RelationalReadModel + RelationalReadModelIncludes,
    {
        self.read_models.sync(model)?;
        Ok(self)
    }

    /// Stage a complete insert. Explicit join read models use this path.
    pub fn insert<M>(&mut self, model: &M) -> Result<&mut Self, TableStoreError>
    where
        M: RelationalReadModel,
    {
        self.read_models.insert(model)?;
        Ok(self)
    }

    /// Stage a complete create-or-save.
    pub fn upsert<M>(&mut self, model: &M) -> Result<&mut Self, TableStoreError>
    where
        M: RelationalReadModel,
    {
        self.read_models.upsert(model)?;
        Ok(self)
    }

    /// Stage a relationship-aware insert through existing ORM metadata.
    pub fn insert_related<P, C>(
        &mut self,
        parent: &P,
        relationship: &str,
        child: &C,
    ) -> Result<&mut Self, TableStoreError>
    where
        P: RelationalReadModel,
        C: RelationalReadModel,
    {
        self.read_models
            .insert_related(parent, relationship, child)?;
        Ok(self)
    }

    /// Stage a relationship-aware create-or-save through existing metadata.
    pub fn upsert_related<P, C>(
        &mut self,
        parent: &P,
        relationship: &str,
        child: &C,
    ) -> Result<&mut Self, TableStoreError>
    where
        P: RelationalReadModel,
        C: RelationalReadModel,
    {
        self.read_models
            .upsert_related(parent, relationship, child)?;
        Ok(self)
    }

    /// Stage an existing-row patch.
    pub fn patch<M>(&mut self, key: RowKey, patch: RowPatch) -> Result<&mut Self, TableStoreError>
    where
        M: RelationalReadModel,
    {
        self.read_models.patch::<M>(key, patch)?;
        Ok(self)
    }

    /// Stage a patch or validated complete create.
    pub fn upsert_patch<M>(
        &mut self,
        key: RowKey,
        patch: RowPatch,
    ) -> Result<&mut Self, TableStoreError>
    where
        M: RelationalReadModel,
    {
        self.read_models.upsert_patch::<M>(key, patch)?;
        Ok(self)
    }

    /// Stage an exact causal delete.
    pub fn delete<M>(&mut self, key: RowKey) -> Result<&mut Self, TableStoreError>
    where
        M: RelationalReadModel,
    {
        self.read_models.delete::<M>(key)?;
        Ok(self)
    }

    /// Stage deletion of one typed model.
    pub fn delete_model<M>(&mut self, model: &M) -> Result<&mut Self, TableStoreError>
    where
        M: RelationalReadModel,
    {
        self.read_models.delete_model(model)?;
        Ok(self)
    }

    /// Return whether no graph diff or explicit row mutation is staged.
    pub fn is_empty(&self) -> bool {
        self.read_models.is_empty()
    }

    pub(super) fn into_execution_parts(
        self,
    ) -> Result<
        (
            TableWritePlan,
            HashMap<ProjectionRecordScope, ProjectionScopedRowSnapshot>,
        ),
        TableStoreError,
    > {
        Ok((self.read_models.into_write_plan()?, self.revisions))
    }

    pub(super) fn belongs_to(&self, causal: &Arc<Mutex<Option<ProjectionWorkspace>>>) -> bool {
        Arc::ptr_eq(&self.causal, causal)
    }

    async fn load_graph<M>(
        &mut self,
        key: RowKey,
        mut includes: Vec<String>,
    ) -> Result<Option<LoadedProjectionGraph<M>>, HandlerError>
    where
        M: RelationalReadModel + RelationalReadModelIncludes,
    {
        includes.sort();
        if includes.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(ProjectionProtocolError::InvalidBatch(format!(
                "projection graph load repeats an include for model `{}`",
                M::schema().model_name
            ))
            .into());
        }
        let mut request = self.request::<M>(key, includes, MAX_PROJECTION_QUERY_BATCH_ROWS)?;
        let scope = request.root.scope.clone();
        let graph = if let Some(cached) = self.graphs.get(&scope) {
            let requested = request.includes.keys().collect::<Vec<_>>();
            let present = cached.includes.keys().collect::<Vec<_>>();
            if requested != present {
                return Err(ProjectionProtocolError::InvalidBatch(format!(
                    "projection graph scope for model `{}` was already loaded with a different include set",
                    scope.model()
                ))
                .into());
            }
            cached.clone()
        } else {
            let cost = request.includes.len().checked_add(1).ok_or_else(|| {
                ProjectionProtocolError::InvalidBatch(
                    "projection graph query-scope count overflowed".into(),
                )
            })?;
            let next = self.query_scope_count.checked_add(cost).ok_or_else(|| {
                ProjectionProtocolError::InvalidBatch(
                    "projection graph cumulative query-scope count overflowed".into(),
                )
            })?;
            if next > MAX_PROJECTION_QUERY_BATCH_ROWS {
                return Err(ProjectionProtocolError::InvalidBatch(format!(
                    "projection graph workspace has {next} query scopes; maximum is {MAX_PROJECTION_QUERY_BATCH_ROWS}"
                ))
                .into());
            }
            request.max_unique_record_scopes =
                self.query_scopes.reserve_graph_root(&request.root.scope)?;
            let graph = self.snapshots.graph_snapshot(&request).await?;
            self.validate_graph::<M>(&request, &graph)?;
            let returned_scopes = graph_record_scopes(&graph)?;
            if let Some(overlap) = returned_scopes
                .iter()
                .find(|scope| self.loaded_record_scopes.contains(*scope))
            {
                return Err(ProjectionProtocolError::InvalidBatch(format!(
                    "projection graph scope for model `{}` was returned by two distinct adapter snapshots",
                    overlap.model()
                ))
                .into());
            }
            self.query_scopes.reserve(returned_scopes.iter())?;
            let mut cumulative = self.loaded_record_scopes.clone();
            cumulative.extend(returned_scopes);
            if cumulative.len() > MAX_PROJECTION_QUERY_BATCH_ROWS {
                return Err(ProjectionProtocolError::InvalidBatch(format!(
                    "projection graph workspace has {} unique record scopes; maximum is {MAX_PROJECTION_QUERY_BATCH_ROWS}",
                    cumulative.len()
                ))
                .into());
            }
            self.query_scope_count = next;
            self.loaded_record_scopes = cumulative;
            self.graphs.insert(scope.clone(), graph.clone());
            graph
        };
        self.hydrate_graph::<M>(graph)
    }

    fn request<M>(
        &self,
        key: RowKey,
        includes: Vec<String>,
        max_unique_record_scopes: usize,
    ) -> Result<ProjectionGraphSnapshotRequest, HandlerError>
    where
        M: RelationalReadModel + RelationalReadModelIncludes,
    {
        self.causal
            .lock()
            .map_err(|_| unavailable_graph_workspace("build graph snapshot"))?
            .as_ref()
            .ok_or_else(|| unavailable_graph_workspace("build graph snapshot"))?
            .graph_snapshot_request::<M>(key, includes, max_unique_record_scopes)
            .map_err(Into::into)
    }

    fn validate_graph<M>(
        &self,
        request: &ProjectionGraphSnapshotRequest,
        graph: &ProjectionGraphSnapshot,
    ) -> Result<(), HandlerError>
    where
        M: RelationalReadModel + RelationalReadModelIncludes,
    {
        if graph.root.scope != request.root.scope {
            return Err(ProjectionProtocolError::ScopeMismatch {
                field: "projection graph root",
            }
            .into());
        }
        validate_snapshot_scope(&graph.root)?;
        if let Some(root_row) = &graph.root.row {
            let root_key = key_from_row(&request.root.schema, root_row)?;
            if root_key != request.root.key {
                return Err(ProjectionProtocolError::ScopeMismatch {
                    field: "projection graph root row key",
                }
                .into());
            }
        }
        let returned_scopes = graph_record_scopes(graph)?;
        if returned_scopes.len() > request.max_unique_record_scopes {
            return Err(ProjectionProtocolError::InvalidBatch(format!(
                "projection graph returned {} unique record scopes; request budget is {}",
                returned_scopes.len(),
                request.max_unique_record_scopes
            ))
            .into());
        }
        if graph.includes.keys().collect::<Vec<_>>() != request.includes.keys().collect::<Vec<_>>()
        {
            return Err(ProjectionProtocolError::InvalidBatch(format!(
                "projection graph model `{}` returned an unknown or missing include",
                M::schema().model_name
            ))
            .into());
        }
        if graph.root.row.is_none()
            && graph
                .includes
                .values()
                .any(|include| !include.rows.is_empty())
        {
            return Err(ProjectionProtocolError::InvalidBatch(format!(
                "projection graph model `{}` returned included rows without a root",
                M::schema().model_name
            ))
            .into());
        }
        let root_row = graph.root.row.as_ref();

        let mut scopes = HashMap::new();
        scopes.insert(graph.root.scope.clone(), &graph.root);
        let causal = self
            .causal
            .lock()
            .map_err(|_| unavailable_graph_workspace("validate graph snapshot"))?;
        let causal = causal
            .as_ref()
            .ok_or_else(|| unavailable_graph_workspace("validate graph snapshot"))?;
        for (name, include) in &graph.includes {
            let expected = request
                .includes
                .get(name)
                .expect("include key sets were compared above");
            if include.relationship != expected.relationship
                || include.target_schema != *expected.target_schema
            {
                return Err(ProjectionProtocolError::InvalidBatch(format!(
                    "projection graph relationship `{name}` returned divergent metadata"
                ))
                .into());
            }
            let target_schema = M::include_target_schema(name)?;
            if target_schema != &include.target_schema {
                return Err(ProjectionProtocolError::InvalidBatch(format!(
                    "projection graph relationship `{name}` returned a divergent typed target schema"
                ))
                .into());
            }
            if matches!(include.relationship.kind, RelationshipKind::BelongsTo)
                && include.rows.len() > 1
            {
                return Err(ProjectionProtocolError::InvalidBatch(format!(
                    "projection graph belongs-to relationship `{name}` returned more than one row"
                ))
                .into());
            }
            for row in &include.rows {
                validate_snapshot_scope(row)?;
                let Some(values) = row.row.as_ref() else {
                    return Err(ProjectionProtocolError::InvalidBatch(format!(
                        "projection graph relationship `{name}` returned a missing or tombstoned included row"
                    ))
                    .into());
                };
                if let Some(root_row) = root_row {
                    validate_relationship_membership(
                        &request.root.schema,
                        root_row,
                        &include.relationship,
                        target_schema,
                        values,
                    )?;
                }
                let key = key_from_row(target_schema, values)?;
                if causal.record_scope(target_schema, &key)? != row.scope {
                    return Err(ProjectionProtocolError::ScopeMismatch {
                        field: "projection graph included row",
                    }
                    .into());
                }
                if let Some(previous) = scopes.get(&row.scope) {
                    if *previous != row {
                        return Err(ProjectionProtocolError::InvalidBatch(format!(
                            "projection graph returns divergent snapshots for model `{}` record scope",
                            row.scope.model()
                        ))
                        .into());
                    }
                } else {
                    scopes.insert(row.scope.clone(), row);
                }
            }
        }
        Ok(())
    }

    fn hydrate_graph<M>(
        &mut self,
        graph: ProjectionGraphSnapshot,
    ) -> Result<Option<LoadedProjectionGraph<M>>, HandlerError>
    where
        M: RelationalReadModel + RelationalReadModelIncludes,
    {
        let (root_row, root_record) = match (&graph.root.row, &graph.root.record) {
            (None, None) => return Ok(None),
            (Some(row), Some(record)) if !record.tombstone => (row.clone(), record.clone()),
            (None, Some(record)) if record.tombstone => {
                return Err(ProjectionProtocolError::RecordTombstoned {
                    model: M::schema().model_name.clone(),
                }
                .into())
            }
            _ => {
                return Err(ProjectionProtocolError::InvalidBatch(format!(
                    "projection graph root `{}` is inconsistent",
                    M::schema().model_name
                ))
                .into())
            }
        };

        let mut model = M::from_row(root_row.clone())?;
        let mut ordinary_includes = BTreeMap::new();
        self.revisions
            .insert(graph.root.scope.clone(), graph.root.clone());
        for (name, include) in graph.includes {
            let rows = include
                .rows
                .iter()
                .map(|snapshot| {
                    let row = snapshot
                        .row
                        .clone()
                        .expect("validated graph includes contain live rows");
                    let record = snapshot
                        .record
                        .as_ref()
                        .expect("validated graph includes contain protocol metadata");
                    Versioned {
                        data: row,
                        version: record.revision.revision(),
                    }
                })
                .collect::<Vec<_>>();
            model.hydrate_include(&name, rows.iter().map(|row| row.data.clone()).collect())?;
            for snapshot in include.rows {
                self.revisions.insert(snapshot.scope.clone(), snapshot);
            }
            ordinary_includes.insert(
                name,
                ReadModelIncludeRows {
                    relationship: include.relationship,
                    target_schema: include.target_schema,
                    rows,
                },
            );
        }
        self.read_models.track_graph::<M>(
            Versioned {
                data: root_row,
                version: root_record.revision.revision(),
            },
            ordinary_includes,
        )?;
        Ok(Some(LoadedProjectionGraph {
            data: model,
            revision: root_record.revision,
        }))
    }
}

fn validate_relationship_membership(
    root_schema: &crate::table::TableSchema,
    root_row: &RowValues,
    relationship: &crate::table::RelationshipDef,
    target_schema: &crate::table::TableSchema,
    target_row: &RowValues,
) -> Result<(), ProjectionProtocolError> {
    let foreign_key = relationship.foreign_key.as_deref().ok_or_else(|| {
        ProjectionProtocolError::InvalidBatch(format!(
            "projection graph relationship `{}` has no foreign key",
            relationship.field_name
        ))
    })?;
    let coherent = match relationship.kind {
        RelationshipKind::HasMany => {
            let (target_column, root_column) =
                crate::projection_protocol::projection_has_many_columns(
                    root_schema,
                    relationship,
                    target_schema,
                )?;
            root_row.get(&root_column) == target_row.get(&target_column)
        }
        RelationshipKind::BelongsTo => {
            let source_column = column_name_for(root_schema, foreign_key).ok_or_else(|| {
                ProjectionProtocolError::InvalidBatch(format!(
                    "projection graph relationship `{}` foreign key `{foreign_key}` is not a source column",
                    relationship.field_name
                ))
            })?;
            if target_schema.primary_key.columns.len() != 1 {
                return Err(ProjectionProtocolError::InvalidBatch(format!(
                    "projection graph belongs-to target `{}` must have one primary-key column",
                    target_schema.model_name
                )));
            }
            root_row.get(&source_column)
                == target_row.get(&target_schema.primary_key.columns[0])
        }
        RelationshipKind::ManyToMany => {
            return Err(ProjectionProtocolError::InvalidBatch(format!(
                "projection graph relationship `{}` is many-to-many; project an explicit join read model instead",
                relationship.field_name
            )))
        }
    };
    if coherent {
        Ok(())
    } else {
        Err(ProjectionProtocolError::InvalidBatch(format!(
            "projection graph relationship `{}` returned a row outside the requested root",
            relationship.field_name
        )))
    }
}

fn graph_record_scopes(
    graph: &ProjectionGraphSnapshot,
) -> Result<HashSet<ProjectionRecordScope>, ProjectionProtocolError> {
    let mut scopes = HashSet::new();
    scopes.insert(graph.root.scope.clone());
    for include in graph.includes.values() {
        for row in &include.rows {
            scopes.insert(row.scope.clone());
        }
    }
    Ok(scopes)
}

/// Builder for one explicit coherent graph load (crate-private).
pub(crate) struct ProjectionGraphLoadBuilder<'workspace, M>
where
    M: RelationalReadModel + RelationalReadModelIncludes,
{
    workspace: &'workspace mut ProjectionReadModelWorkspace,
    key: RowKey,
    includes: Vec<String>,
    marker: PhantomData<M>,
}

impl<'workspace, M> ProjectionGraphLoadBuilder<'workspace, M>
where
    M: RelationalReadModel + RelationalReadModelIncludes,
{
    /// Include one declared relationship in the coherent adapter snapshot.
    pub fn include(mut self, relationship: impl Into<String>) -> Self {
        self.includes.push(relationship.into());
        self
    }

    /// Load and hydrate at most one root graph.
    pub async fn one(self) -> Result<Option<LoadedProjectionGraph<M>>, HandlerError> {
        self.workspace
            .load_graph::<M>(self.key, self.includes)
            .await
    }
}

fn unavailable_graph_workspace(operation: &'static str) -> HandlerError {
    ProjectionProtocolError::InvalidBatch(format!(
        "causal projector workspace is unavailable while attempting to {operation}"
    ))
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::projection_protocol::{
        ProjectionChangeCursor, ProjectionEpoch, ProjectionExecutionSnapshotBatch,
        ProjectionExecutionSnapshotBatchRequest, ProjectionGeneration,
        ProjectionGraphIncludeSnapshot, ProjectionInputCursor, ProjectionInputFingerprint,
        ProjectionPartition, ProjectionQuerySnapshot, ProjectionQuerySnapshotRequest,
        ProjectionRecordMetadata, ProjectionScopeCodec, ProjectionSource, ProjectorTopologyId,
        TrustedProjectionInput,
    };
    use crate::read_model::RelationalReadModel;
    use crate::table::{ExpectedVersion, RowValue, RowValues, TableMutation};

    #[derive(
        Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize, crate::ReadModel,
    )]
    #[readmodel(table = "graph_players", primary_key = ["player_id"])]
    struct Player {
        player_id: String,
        display_name: String,
        #[readmodel(has_many = "PlayerWeapon", foreign_key = "player_id")]
        weapons: Vec<PlayerWeapon>,
    }

    #[derive(
        Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize, crate::ReadModel,
    )]
    #[readmodel(
        table = "graph_player_weapons",
        primary_key = ["player_id", "weapon_id"]
    )]
    struct PlayerWeapon {
        #[readmodel(
            foreign_key = "graph_players.player_id",
            delegated_from = "Player.player_id"
        )]
        player_id: String,
        weapon_id: String,
        acquired_at: String,
        #[readmodel(belongs_to = "Player", foreign_key = "player_id")]
        player: Option<Player>,
    }

    #[derive(
        Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize, crate::ReadModel,
    )]
    #[readmodel(
        table = "graph_player_weapon_links",
        primary_key = ["player_id", "weapon_id"]
    )]
    struct PlayerWeaponLink {
        player_id: String,
        weapon_id: String,
    }

    #[derive(Clone)]
    struct FakeSnapshots {
        graph_results: Arc<Mutex<VecDeque<ProjectionGraphSnapshot>>>,
        single_calls: Arc<AtomicUsize>,
        graph_calls: Arc<AtomicUsize>,
        execution_calls: Arc<AtomicUsize>,
        execution_scope_counts: Arc<Mutex<Vec<usize>>>,
    }

    impl FakeSnapshots {
        fn new(graphs: impl IntoIterator<Item = ProjectionGraphSnapshot>) -> Self {
            Self {
                graph_results: Arc::new(Mutex::new(graphs.into_iter().collect())),
                single_calls: Arc::new(AtomicUsize::new(0)),
                graph_calls: Arc::new(AtomicUsize::new(0)),
                execution_calls: Arc::new(AtomicUsize::new(0)),
                execution_scope_counts: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    impl ProjectionSnapshotReader for FakeSnapshots {
        fn snapshot<'a>(
            &'a self,
            _request: &'a ProjectionQuerySnapshotRequest,
        ) -> Pin<
            Box<
                dyn Future<Output = Result<ProjectionQuerySnapshot, ProjectionProtocolError>>
                    + Send
                    + 'a,
            >,
        > {
            self.single_calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async {
                Ok(ProjectionQuerySnapshot {
                    row: None,
                    record: None,
                    checkpoints: Vec::new(),
                    change_head: None,
                    compacted_through: 0,
                })
            })
        }

        fn execution_snapshots<'a>(
            &'a self,
            request: &'a ProjectionExecutionSnapshotBatchRequest,
        ) -> Pin<
            Box<
                dyn Future<
                        Output = Result<ProjectionExecutionSnapshotBatch, ProjectionProtocolError>,
                    > + Send
                    + 'a,
            >,
        > {
            self.execution_calls.fetch_add(1, Ordering::SeqCst);
            self.execution_scope_counts
                .lock()
                .unwrap()
                .push(request.requests.len());
            let snapshots = request
                .requests
                .iter()
                .map(|request| ProjectionScopedRowSnapshot {
                    scope: request.scope.clone(),
                    row: None,
                    record: None,
                })
                .collect();
            Box::pin(async move { Ok(ProjectionExecutionSnapshotBatch { snapshots }) })
        }

        fn graph_snapshot<'a>(
            &'a self,
            _request: &'a ProjectionGraphSnapshotRequest,
        ) -> Pin<
            Box<
                dyn Future<Output = Result<ProjectionGraphSnapshot, ProjectionProtocolError>>
                    + Send
                    + 'a,
            >,
        > {
            self.graph_calls.fetch_add(1, Ordering::SeqCst);
            let result = self
                .graph_results
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| {
                    ProjectionProtocolError::InvalidBatch(
                        "fake graph snapshot queue was exhausted".into(),
                    )
                });
            Box::pin(async move { result })
        }
    }

    fn topology() -> ProjectorTopologyId {
        ProjectorTopologyId::new(1, "graph-workspace-tests", [41; 32]).unwrap()
    }

    fn causal_workspace() -> ProjectionWorkspace {
        let topology = topology();
        let mut codec = ProjectionScopeCodec::new(topology.clone());
        for (name, schema) in [
            ("Player", Player::schema()),
            ("PlayerWeapon", PlayerWeapon::schema()),
            ("PlayerWeaponLink", PlayerWeaponLink::schema()),
        ] {
            codec.register_model(name, schema).unwrap();
        }
        let partition = codec.encode_partition(None).unwrap();
        let input = TrustedProjectionInput::mint(
            ProjectionInputCursor::new(
                topology,
                partition,
                ProjectionSource::new("graph-source", b"player-1".to_vec()).unwrap(),
                ProjectionEpoch::new("source-v1").unwrap(),
                1,
            )
            .unwrap(),
            ProjectionInputFingerprint::from_canonical_bytes(b"graph-input"),
            "message-1",
            "cause-1",
            ProjectionGeneration::initial(),
            true,
        )
        .unwrap();
        ProjectionWorkspace::new(
            Arc::new(codec),
            None,
            input,
            ProjectionEpoch::new("changes-v1").unwrap(),
        )
        .unwrap()
    }

    fn player_key(id: &str) -> RowKey {
        RowKey::new([("player_id", RowValue::String(id.into()))])
    }

    fn weapon_key(player_id: &str, weapon_id: &str) -> RowKey {
        RowKey::new([
            ("player_id", RowValue::String(player_id.into())),
            ("weapon_id", RowValue::String(weapon_id.into())),
        ])
    }

    fn player(id: &str, name: &str) -> Player {
        Player {
            player_id: id.into(),
            display_name: name.into(),
            weapons: Vec::new(),
        }
    }

    fn weapon(player_id: &str, weapon_id: &str, acquired_at: &str) -> PlayerWeapon {
        PlayerWeapon {
            player_id: player_id.into(),
            weapon_id: weapon_id.into(),
            acquired_at: acquired_at.into(),
            player: None,
        }
    }

    fn partition() -> ProjectionPartition {
        ProjectionScopeCodec::new(topology())
            .encode_partition(None)
            .unwrap()
    }

    fn metadata(scope: ProjectionRecordScope, revision: u64) -> ProjectionRecordMetadata {
        ProjectionRecordMetadata {
            revision: RecordRevision::new(scope, 1, revision).unwrap(),
            tombstone: false,
            change: ProjectionChangeCursor::new(
                topology(),
                partition(),
                ProjectionEpoch::new("changes-v1").unwrap(),
                revision,
            )
            .unwrap(),
        }
    }

    fn live_snapshot(
        causal: &ProjectionWorkspace,
        schema: &'static crate::table::TableSchema,
        key: RowKey,
        row: RowValues,
        revision: u64,
    ) -> ProjectionScopedRowSnapshot {
        let scope = causal.record_scope(schema, &key).unwrap();
        ProjectionScopedRowSnapshot {
            row: Some(row),
            record: Some(metadata(scope.clone(), revision)),
            scope,
        }
    }

    fn player_graph(
        causal: &ProjectionWorkspace,
        root: Player,
        children: Vec<PlayerWeapon>,
    ) -> ProjectionGraphSnapshot {
        let root = live_snapshot(
            causal,
            Player::schema(),
            player_key(&root.player_id),
            root.to_row().unwrap(),
            7,
        );
        let rows = children
            .into_iter()
            .enumerate()
            .map(|(index, child)| {
                live_snapshot(
                    causal,
                    PlayerWeapon::schema(),
                    weapon_key(&child.player_id, &child.weapon_id),
                    child.to_row().unwrap(),
                    10 + index as u64,
                )
            })
            .collect();
        let relationship = Player::schema()
            .relationships
            .iter()
            .find(|relationship| relationship.field_name == "weapons")
            .unwrap()
            .clone();
        ProjectionGraphSnapshot {
            root,
            includes: BTreeMap::from([(
                "weapons".into(),
                ProjectionGraphIncludeSnapshot {
                    relationship,
                    target_schema: PlayerWeapon::schema().clone(),
                    rows,
                },
            )]),
        }
    }

    fn weapon_graph(
        causal: &ProjectionWorkspace,
        root: PlayerWeapon,
        owner: Player,
        root_revision: u64,
        owner_revision: u64,
    ) -> ProjectionGraphSnapshot {
        let root = live_snapshot(
            causal,
            PlayerWeapon::schema(),
            weapon_key(&root.player_id, &root.weapon_id),
            root.to_row().unwrap(),
            root_revision,
        );
        let owner = live_snapshot(
            causal,
            Player::schema(),
            player_key(&owner.player_id),
            owner.to_row().unwrap(),
            owner_revision,
        );
        let relationship = PlayerWeapon::schema()
            .relationships
            .iter()
            .find(|relationship| relationship.field_name == "player")
            .unwrap()
            .clone();
        ProjectionGraphSnapshot {
            root,
            includes: BTreeMap::from([(
                "player".into(),
                ProjectionGraphIncludeSnapshot {
                    relationship,
                    target_schema: Player::schema().clone(),
                    rows: vec![owner],
                },
            )]),
        }
    }

    #[tokio::test]
    async fn coherent_graph_sync_reuses_exact_revisions_and_has_many_delete_semantics() {
        let causal = causal_workspace();
        let graph = player_graph(
            &causal,
            player("player-1", "Ada"),
            vec![
                weapon("player-1", "shield", "2026-07-27"),
                weapon("player-1", "sword", "2026-07-28"),
            ],
        );
        let reader = FakeSnapshots::new([graph]);
        let causal_slot = Arc::new(Mutex::new(Some(causal)));
        let mut read_models =
            ProjectionReadModelWorkspace::new(Arc::new(reader.clone()), Arc::clone(&causal_slot));
        let mut loaded = read_models
            .load::<Player>(player_key("player-1"))
            .include("weapons")
            .one()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded.revision.revision(), 7);
        loaded.data.display_name = "Ada Lovelace".into();
        loaded
            .data
            .weapons
            .retain(|weapon| weapon.weapon_id == "sword");
        read_models.sync(loaded.data).unwrap();

        let (plan, cached) = read_models.into_execution_parts().unwrap();
        assert_eq!(plan.mutations.len(), 2);
        assert!(plan.mutations.iter().any(|mutation| {
            matches!(
                mutation,
                TableMutation::DeleteRow(delete)
                    if delete.schema.table_name == "graph_player_weapons"
                        && delete.expected_version == ExpectedVersion::Exact(10)
            )
        }));
        let prepared = {
            let causal = causal_slot.lock().unwrap();
            crate::projection::executor::prepare_graph_projection(
                causal.as_ref().unwrap(),
                plan,
                cached,
            )
            .unwrap()
        };
        assert!(
            !prepared.needs_snapshot_read(),
            "root and included revisions came from one graph snapshot"
        );
        prepared
            .stage(
                causal_slot.lock().unwrap().as_mut().unwrap(),
                ProjectionExecutionSnapshotBatch::default(),
            )
            .unwrap();
        let batch = causal_slot
            .lock()
            .unwrap()
            .take()
            .unwrap()
            .into_batch()
            .unwrap();
        assert_eq!(batch.mutations.len(), 2);
        assert!(batch.mutations.iter().all(|mutation| {
            match &mutation.mutation {
                TableMutation::PatchRow(patch) => patch.expected_version == ExpectedVersion::Any,
                TableMutation::DeleteRow(delete) => delete.expected_version == ExpectedVersion::Any,
                TableMutation::UpsertRow(_) => false,
            }
        }));
        assert_eq!(reader.graph_calls.load(Ordering::SeqCst), 1);
        assert_eq!(reader.execution_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn unchanged_graph_and_belongs_to_clear_do_not_invent_writes_or_observations() {
        let causal = causal_workspace();
        let root_weapon = weapon("player-1", "sword", "2026-07-28");
        let root = live_snapshot(
            &causal,
            PlayerWeapon::schema(),
            weapon_key("player-1", "sword"),
            root_weapon.to_row().unwrap(),
            20,
        );
        let owner = player("player-1", "Ada");
        let owner_snapshot = live_snapshot(
            &causal,
            Player::schema(),
            player_key("player-1"),
            owner.to_row().unwrap(),
            21,
        );
        let relationship = PlayerWeapon::schema()
            .relationships
            .iter()
            .find(|relationship| relationship.field_name == "player")
            .unwrap()
            .clone();
        let graph = ProjectionGraphSnapshot {
            root,
            includes: BTreeMap::from([(
                "player".into(),
                ProjectionGraphIncludeSnapshot {
                    relationship,
                    target_schema: Player::schema().clone(),
                    rows: vec![owner_snapshot],
                },
            )]),
        };
        let reader = FakeSnapshots::new([graph]);
        let causal_slot = Arc::new(Mutex::new(Some(causal)));
        let mut read_models =
            ProjectionReadModelWorkspace::new(Arc::new(reader.clone()), Arc::clone(&causal_slot));
        let mut loaded = read_models
            .load::<PlayerWeapon>(weapon_key("player-1", "sword"))
            .include("player")
            .one()
            .await
            .unwrap()
            .unwrap()
            .data;
        assert!(loaded.player.is_some());
        loaded.player = None;
        read_models.sync(loaded).unwrap();
        let (plan, cached) = read_models.into_execution_parts().unwrap();
        assert!(
            plan.mutations.is_empty(),
            "clearing belongs-to hydration must not delete its target"
        );
        let prepared = {
            let causal = causal_slot.lock().unwrap();
            crate::projection::executor::prepare_graph_projection(
                causal.as_ref().unwrap(),
                plan,
                cached,
            )
            .unwrap()
        };
        prepared
            .stage(
                causal_slot.lock().unwrap().as_mut().unwrap(),
                ProjectionExecutionSnapshotBatch::default(),
            )
            .unwrap();
        let batch = causal_slot
            .lock()
            .unwrap()
            .take()
            .unwrap()
            .into_batch()
            .unwrap();
        assert!(batch.mutations.is_empty());
        assert!(batch.observations.is_empty());
        assert_eq!(reader.graph_calls.load(Ordering::SeqCst), 1);
        assert_eq!(reader.execution_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn public_apply_batches_multi_table_and_explicit_join_scopes_once() {
        let causal = causal_workspace();
        let reader = FakeSnapshots::new([]);
        let message =
            crate::bus::Message::new("player.changed", crate::bus::MessageKind::Event, Vec::new())
                .with_id("message-1")
                .with_metadata(crate::trace_context::CAUSATION_ID, "cause-1");
        let (context, staged) =
            super::super::context::CausalProjectorContext::new(&message, reader.clone(), causal);
        let mut read_models = context.read_models();
        read_models
            .upsert(&player("player-1", "Ada"))
            .unwrap()
            .upsert(&weapon("player-1", "sword", "2026-07-28"))
            .unwrap()
            .insert(&PlayerWeaponLink {
                player_id: "player-1".into(),
                weapon_id: "sword".into(),
            })
            .unwrap();

        context.apply(read_models).await.unwrap();

        assert_eq!(reader.execution_calls.load(Ordering::SeqCst), 1);
        assert_eq!(*reader.execution_scope_counts.lock().unwrap(), vec![3]);
        let batch = staged.lock().unwrap().take().unwrap().into_batch().unwrap();
        assert_eq!(batch.mutations.len(), 3);
        assert_eq!(
            batch
                .mutations
                .iter()
                .map(|mutation| mutation.mutation.table_name())
                .collect::<Vec<_>>(),
            vec![
                "graph_player_weapon_links",
                "graph_players",
                "graph_player_weapons"
            ],
            "the canonical ORM plan order is preserved through causal staging"
        );
        assert!(batch.mutations.iter().all(|mutation| {
            matches!(
                &mutation.mutation,
                TableMutation::UpsertRow(row)
                    if row.mode == crate::table::RowWriteMode::Insert
                        && row.expected_version == ExpectedVersion::NotExists
            )
        }));
    }

    #[tokio::test]
    async fn graph_rejects_an_included_row_outside_the_requested_root() {
        let causal = causal_workspace();
        let graph = player_graph(
            &causal,
            player("player-1", "Ada"),
            vec![weapon("player-2", "sword", "2026-07-28")],
        );
        let reader = FakeSnapshots::new([graph]);
        let mut read_models = ProjectionReadModelWorkspace::new(
            Arc::new(reader.clone()),
            Arc::new(Mutex::new(Some(causal))),
        );
        let error = read_models
            .load::<Player>(player_key("player-1"))
            .include("weapons")
            .one()
            .await
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("returned a row outside the requested root"));
        assert_eq!(reader.graph_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn distinct_graph_snapshots_cannot_replace_an_older_shared_revision_sidecar() {
        let causal = causal_workspace();
        let graph_a = weapon_graph(
            &causal,
            weapon("player-1", "sword", "2026-07-28"),
            player("player-1", "Ada"),
            20,
            21,
        );
        let graph_b = weapon_graph(
            &causal,
            weapon("player-1", "shield", "2026-07-29"),
            player("player-1", "Ada v2"),
            22,
            23,
        );
        let reader = FakeSnapshots::new([graph_a, graph_b]);
        let causal_slot = Arc::new(Mutex::new(Some(causal)));
        let mut read_models =
            ProjectionReadModelWorkspace::new(Arc::new(reader.clone()), Arc::clone(&causal_slot));
        let mut first = read_models
            .load::<PlayerWeapon>(weapon_key("player-1", "sword"))
            .include("player")
            .one()
            .await
            .unwrap()
            .unwrap()
            .data;
        let error = read_models
            .load::<PlayerWeapon>(weapon_key("player-1", "shield"))
            .include("player")
            .one()
            .await
            .unwrap_err();
        assert!(error.to_string().contains("two distinct adapter snapshots"));

        first.player.as_mut().unwrap().display_name = "Countess Lovelace".into();
        read_models.sync(first).unwrap();
        let (plan, cached) = read_models.into_execution_parts().unwrap();
        assert!(plan.mutations.iter().any(|mutation| {
            matches!(
                mutation,
                TableMutation::PatchRow(patch)
                    if patch.schema.model_name == "Player"
                        && patch.expected_version == ExpectedVersion::Exact(21)
            )
        }));
        let owner_scope = causal_slot
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .record_scope(Player::schema(), &player_key("player-1"))
            .unwrap();
        assert_eq!(
            cached[&owner_scope]
                .record
                .as_ref()
                .unwrap()
                .revision
                .revision(),
            21,
            "the rejected second snapshot must not replace the first baseline's sidecar"
        );
    }

    #[tokio::test]
    async fn context_unique_query_scope_limit_fails_before_call_4097() {
        let reader = FakeSnapshots::new([]);
        let message =
            crate::bus::Message::new("player.changed", crate::bus::MessageKind::Event, Vec::new())
                .with_id("message-1")
                .with_metadata(crate::trace_context::CAUSATION_ID, "cause-1");
        let (context, _) = super::super::context::CausalProjectorContext::new(
            &message,
            reader.clone(),
            causal_workspace(),
        );

        for index in 0..MAX_PROJECTION_QUERY_BATCH_ROWS {
            let id = format!("player-{index}");
            assert!(context
                .load::<Player>(player_key(&id))
                .await
                .unwrap()
                .is_none());
        }
        let error = context
            .load::<Player>(player_key("player-over-limit"))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("4097 unique query scopes"));
        assert_eq!(
            reader.single_calls.load(Ordering::SeqCst),
            MAX_PROJECTION_QUERY_BATCH_ROWS,
            "scope 4097 must fail before adapter I/O"
        );
    }

    #[test]
    fn graph_record_scope_budget_accepts_4096_and_rejects_hostile_over_limit_results() {
        let causal = causal_workspace();
        let request = causal
            .graph_snapshot_request::<Player>(
                player_key("player-1"),
                vec!["weapons".into()],
                MAX_PROJECTION_QUERY_BATCH_ROWS,
            )
            .unwrap();
        let root_scope = request.root.scope.clone();
        let rows = (0..MAX_PROJECTION_QUERY_BATCH_ROWS)
            .map(|index| ProjectionScopedRowSnapshot {
                scope: ProjectionRecordScope::new(
                    topology(),
                    partition(),
                    "PlayerWeapon",
                    format!("hostile-child-{index}").into_bytes(),
                )
                .unwrap(),
                row: None,
                record: None,
            })
            .collect::<Vec<_>>();
        let graph = ProjectionGraphSnapshot {
            root: ProjectionScopedRowSnapshot {
                scope: root_scope,
                row: None,
                record: None,
            },
            includes: BTreeMap::from([(
                "weapons".into(),
                ProjectionGraphIncludeSnapshot {
                    relationship: request.includes["weapons"].relationship.clone(),
                    target_schema: PlayerWeapon::schema().clone(),
                    rows,
                },
            )]),
        };
        assert_eq!(
            graph_record_scopes(&ProjectionGraphSnapshot {
                root: graph.root.clone(),
                includes: BTreeMap::from([(
                    "weapons".into(),
                    ProjectionGraphIncludeSnapshot {
                        relationship: graph.includes["weapons"].relationship.clone(),
                        target_schema: graph.includes["weapons"].target_schema.clone(),
                        rows: graph.includes["weapons"].rows[..MAX_PROJECTION_QUERY_BATCH_ROWS - 1]
                            .to_vec(),
                    },
                )]),
            })
            .unwrap()
            .len(),
            MAX_PROJECTION_QUERY_BATCH_ROWS
        );
        let reader = FakeSnapshots::new([]);
        let workspace =
            ProjectionReadModelWorkspace::new(Arc::new(reader), Arc::new(Mutex::new(Some(causal))));
        let error = workspace
            .validate_graph::<Player>(&request, &graph)
            .unwrap_err();
        assert!(error.to_string().contains("4097 unique record scopes"));
        assert!(ProjectionGraphSnapshotRequest::new(
            request.root,
            std::iter::empty(),
            MAX_PROJECTION_QUERY_BATCH_ROWS + 1,
        )
        .is_err());
    }

    #[test]
    fn graph_record_scope_budget_counts_one_target_shared_by_two_includes_once() {
        let causal = causal_workspace();
        let graph = player_graph(
            &causal,
            player("player-1", "Ada"),
            vec![weapon("player-1", "sword", "2026-07-28")],
        );
        let shared = graph.includes["weapons"].clone();
        let graph = ProjectionGraphSnapshot {
            root: graph.root,
            includes: BTreeMap::from([
                ("equipped_weapons".into(), shared.clone()),
                ("weapons".into(), shared),
            ]),
        };

        assert_eq!(
            graph_record_scopes(&graph).unwrap().len(),
            2,
            "the root and one shared target consume two unique record scopes"
        );
    }

    #[test]
    fn inferred_many_to_many_graph_load_is_rejected_in_favor_of_an_explicit_join_model() {
        let causal = causal_workspace();
        let mut root = causal
            .query_snapshot_request::<Player>(player_key("player-1"))
            .unwrap();
        Arc::make_mut(&mut root.schema).relationships[0].kind = RelationshipKind::ManyToMany;
        let error = ProjectionGraphSnapshotRequest::new(
            root,
            [("weapons".into(), Arc::new(PlayerWeapon::schema().clone()))],
            MAX_PROJECTION_QUERY_BATCH_ROWS,
        )
        .unwrap_err();
        assert!(error.to_string().contains("explicit join read model"));
    }
}
