//! Framework-owned staging workspace for causal projectors.
//!
//! Handlers may describe row transitions, but they never receive a repository
//! commit capability and cannot mint checkpoints, revisions, or observations.
//! The workspace lowers typed model keys through the registered scope codec and
//! seals the complete batch for the adapter after the handler returns.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use super::{
    ProjectionCommitBatch, ProjectionEpoch, ProjectionModelOwnership, ProjectionMutationKind,
    ProjectionObservationKind, ProjectionObservationRequest, ProjectionObservationTarget,
    ProjectionProtocolError, ProjectionQuerySnapshotRequest, ProjectionRecordExpectation,
    ProjectionRecordMutation, ProjectionRecordScope, ProjectionScopeCodec, RecordRevision,
    TrustedProjectionInput,
};
use crate::read_model::RelationalReadModel;
use crate::table::{
    DeleteTableRowMutation, ExpectedVersion, PatchMode, PatchTableRowMutation, RowKey, RowPatch,
    RowWriteMode, TableMutation, TableRowMutation, TableSchema,
};

/// Typed, commit-less workspace passed to a causal projector handler.
///
/// Every staged record is automatically observed for the input's causation.
/// Declaration-owned dependency/existing-record observations are attached only
/// through crate-private framework methods.
#[derive(Debug)]
pub struct ProjectionWorkspace {
    codec: Arc<ProjectionScopeCodec>,
    partition_value: Option<serde_json::Value>,
    input: TrustedProjectionInput,
    change_epoch: ProjectionEpoch,
    ownership: BTreeMap<String, String>,
    mutations: Vec<ProjectionRecordMutation>,
    observations: Vec<ProjectionObservationRequest>,
    staged_scopes: HashSet<ProjectionRecordScope>,
}

impl ProjectionWorkspace {
    pub(crate) fn new(
        codec: Arc<ProjectionScopeCodec>,
        partition_value: Option<serde_json::Value>,
        input: TrustedProjectionInput,
        change_epoch: ProjectionEpoch,
    ) -> Result<Self, ProjectionProtocolError> {
        if codec.topology() != input.cursor.topology() {
            return Err(ProjectionProtocolError::ScopeMismatch {
                field: "projection workspace topology",
            });
        }
        let partition = codec
            .encode_partition(partition_value.as_ref())
            .map_err(|error| ProjectionProtocolError::InvalidBatch(error.to_string()))?;
        if &partition != input.cursor.projection_partition() {
            return Err(ProjectionProtocolError::ScopeMismatch {
                field: "projection workspace partition",
            });
        }
        Ok(Self {
            codec,
            partition_value,
            input,
            change_epoch,
            ownership: BTreeMap::new(),
            mutations: Vec::new(),
            observations: Vec::new(),
            staged_scopes: HashSet::new(),
        })
    }

    /// Stage a record that must not have prior protocol metadata.
    pub fn create<M>(&mut self, model: &M) -> Result<&mut Self, ProjectionProtocolError>
    where
        M: RelationalReadModel,
    {
        let schema = M::schema();
        schema.validate()?;
        let key = model.primary_key()?;
        let scope = self.scope(schema, &key)?;
        let mutation = TableMutation::UpsertRow(TableRowMutation {
            schema,
            key,
            values: model.to_row()?,
            expected_version: ExpectedVersion::NotExists,
            mode: RowWriteMode::Insert,
        });
        self.stage(
            schema,
            scope,
            mutation,
            ProjectionRecordExpectation::Missing,
            ProjectionMutationKind::Upsert,
        )
    }

    /// Stage a full-row update fenced by the exact protocol revision previously
    /// loaded by the framework.
    pub fn save<M>(
        &mut self,
        model: &M,
        expected: &RecordRevision,
    ) -> Result<&mut Self, ProjectionProtocolError>
    where
        M: RelationalReadModel,
    {
        let schema = M::schema();
        schema.validate()?;
        let key = model.primary_key()?;
        let scope = self.scope(schema, &key)?;
        let mutation = TableMutation::UpsertRow(TableRowMutation {
            schema,
            key,
            values: model.to_row()?,
            // The framework revision is the authoritative multi-source fence.
            // The partition transaction serializes physical row writes.
            expected_version: ExpectedVersion::Any,
            mode: RowWriteMode::Upsert,
        });
        self.stage(
            schema,
            scope,
            mutation,
            ProjectionRecordExpectation::Exact(expected.clone()),
            ProjectionMutationKind::Upsert,
        )
    }

    /// Stage a sparse update fenced by an exact protocol revision.
    pub fn patch<M>(
        &mut self,
        key: RowKey,
        patch: RowPatch,
        expected: &RecordRevision,
    ) -> Result<&mut Self, ProjectionProtocolError>
    where
        M: RelationalReadModel,
    {
        let schema = M::schema();
        schema.validate()?;
        let scope = self.scope(schema, &key)?;
        let mutation = TableMutation::PatchRow(PatchTableRowMutation {
            schema,
            key,
            patch,
            expected_version: ExpectedVersion::Any,
            mode: PatchMode::UpdateExisting,
        });
        self.stage(
            schema,
            scope,
            mutation,
            ProjectionRecordExpectation::Exact(expected.clone()),
            ProjectionMutationKind::Upsert,
        )
    }

    /// Stage a durable tombstone and physical row deletion.
    pub fn delete<M>(
        &mut self,
        key: RowKey,
        expected: &RecordRevision,
    ) -> Result<&mut Self, ProjectionProtocolError>
    where
        M: RelationalReadModel,
    {
        let schema = M::schema();
        schema.validate()?;
        let scope = self.scope(schema, &key)?;
        let mutation = TableMutation::DeleteRow(DeleteTableRowMutation {
            schema,
            key,
            expected_version: ExpectedVersion::Any,
        });
        self.stage(
            schema,
            scope,
            mutation,
            ProjectionRecordExpectation::Exact(expected.clone()),
            ProjectionMutationKind::Delete,
        )
    }

    /// Explicitly recreate a deleted record from its exact tombstone revision.
    /// The adapter advances the incarnation atomically; ordinary `save` cannot
    /// cross a tombstone.
    pub fn recreate<M>(
        &mut self,
        model: &M,
        expected_tombstone: &RecordRevision,
    ) -> Result<&mut Self, ProjectionProtocolError>
    where
        M: RelationalReadModel,
    {
        let schema = M::schema();
        schema.validate()?;
        let key = model.primary_key()?;
        let scope = self.scope(schema, &key)?;
        let mutation = TableMutation::UpsertRow(TableRowMutation {
            schema,
            key,
            values: model.to_row()?,
            expected_version: ExpectedVersion::NotExists,
            mode: RowWriteMode::Insert,
        });
        self.stage(
            schema,
            scope,
            mutation,
            ProjectionRecordExpectation::Exact(expected_tombstone.clone()),
            ProjectionMutationKind::Recreate,
        )
    }

    pub fn is_empty(&self) -> bool {
        self.mutations.is_empty() && self.observations.is_empty()
    }

    /// Derive a physical-row + protocol snapshot request from this workspace's
    /// exact compiled codec and partition. The handler can supply only a typed
    /// model key; it cannot pair an arbitrary row with independently minted
    /// revision/checkpoint scope bytes.
    pub(crate) fn query_snapshot_request<M>(
        &self,
        key: RowKey,
    ) -> Result<ProjectionQuerySnapshotRequest, ProjectionProtocolError>
    where
        M: RelationalReadModel,
    {
        self.ensure_registered_schema(M::schema())?;
        ProjectionQuerySnapshotRequest::new(
            &self.codec,
            self.partition_value.as_ref(),
            &M::schema().model_name,
            key,
            Vec::new(),
        )
    }

    #[allow(dead_code)]
    pub(crate) fn confirm_existing(
        &mut self,
        schema: &'static TableSchema,
        revision: RecordRevision,
    ) -> Result<&mut Self, ProjectionProtocolError> {
        self.register_ownership(schema)?;
        self.observations.push(ProjectionObservationRequest {
            kind: ProjectionObservationKind::Record,
            target: ProjectionObservationTarget::ExistingRecord(revision),
        });
        Ok(self)
    }

    #[allow(dead_code)]
    pub(crate) fn confirm_dependency(
        &mut self,
        schema: &'static TableSchema,
        key: &RowKey,
    ) -> Result<&mut Self, ProjectionProtocolError> {
        let scope = self.scope(schema, key)?;
        self.observations.push(ProjectionObservationRequest {
            kind: ProjectionObservationKind::Dependency,
            target: ProjectionObservationTarget::Dependency(scope),
        });
        Ok(self)
    }

    pub(crate) fn ownership(
        &self,
    ) -> Result<Vec<ProjectionModelOwnership>, ProjectionProtocolError> {
        self.ownership
            .iter()
            .map(|(model, table)| ProjectionModelOwnership::new(model.clone(), table.clone()))
            .collect()
    }

    pub(crate) fn into_batch(self) -> Result<ProjectionCommitBatch, ProjectionProtocolError> {
        let ownership = self.ownership()?;
        let batch = ProjectionCommitBatch {
            input: self.input,
            change_epoch: self.change_epoch,
            ownership,
            mutations: self.mutations,
            observations: self.observations,
        };
        batch.validate()?;
        Ok(batch)
    }

    fn scope(
        &mut self,
        schema: &'static TableSchema,
        key: &RowKey,
    ) -> Result<ProjectionRecordScope, ProjectionProtocolError> {
        self.register_ownership(schema)?;
        self.codec
            .encode_row_scope(
                self.codec.topology().name(),
                &schema.model_name,
                self.partition_value.as_ref(),
                key,
            )
            .map_err(|error| ProjectionProtocolError::InvalidBatch(error.to_string()))
    }

    fn register_ownership(
        &mut self,
        schema: &'static TableSchema,
    ) -> Result<(), ProjectionProtocolError> {
        self.ensure_registered_schema(schema)?;
        match self
            .ownership
            .insert(schema.model_name.clone(), schema.table_name.clone())
        {
            Some(table) if table != schema.table_name => {
                Err(ProjectionProtocolError::InvalidBatch(format!(
                    "projection model `{}` was staged for both table `{table}` and `{}`",
                    schema.model_name, schema.table_name
                )))
            }
            _ => Ok(()),
        }
    }

    fn ensure_registered_schema(
        &self,
        schema: &'static TableSchema,
    ) -> Result<(), ProjectionProtocolError> {
        let registered = self
            .codec
            .registered_schema(&schema.model_name)
            .map_err(|error| ProjectionProtocolError::InvalidBatch(error.to_string()))?;
        if registered != schema {
            return Err(ProjectionProtocolError::InvalidBatch(format!(
                "projection model `{}` schema differs from the exact compiled topology",
                schema.model_name
            )));
        }
        Ok(())
    }

    fn stage(
        &mut self,
        schema: &'static TableSchema,
        scope: ProjectionRecordScope,
        mutation: TableMutation,
        expectation: ProjectionRecordExpectation,
        kind: ProjectionMutationKind,
    ) -> Result<&mut Self, ProjectionProtocolError> {
        if !self.staged_scopes.insert(scope.clone()) {
            return Err(ProjectionProtocolError::InvalidBatch(format!(
                "projection workspace repeats model `{}` record scope",
                scope.model()
            )));
        }
        self.register_ownership(schema)?;
        self.mutations.push(ProjectionRecordMutation::new(
            scope.clone(),
            mutation,
            expectation,
            kind,
        )?);
        self.observations.push(ProjectionObservationRequest {
            kind: ProjectionObservationKind::Record,
            target: ProjectionObservationTarget::StagedRecord(scope),
        });
        Ok(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projection_protocol::{
        ProjectionGeneration, ProjectionInputCursor, ProjectionInputFingerprint, ProjectionSource,
        ProjectorTopologyId,
    };
    use crate::table::{ColumnType, PrimaryKey, RowValue, RowValues, TableColumn, TableKind};

    #[derive(Clone)]
    struct TodoView {
        id: u64,
        title: String,
    }

    impl RelationalReadModel for TodoView {
        fn schema() -> &'static TableSchema {
            static SCHEMA: std::sync::OnceLock<TableSchema> = std::sync::OnceLock::new();
            SCHEMA.get_or_init(|| TableSchema {
                model_name: "TodoView".into(),
                table_name: "todo_views".into(),
                columns: vec![
                    TableColumn {
                        primary_key: true,
                        ..TableColumn::new("id", "todo_id", ColumnType::UnsignedInteger)
                    },
                    TableColumn::new("title", "title", ColumnType::Text),
                ],
                primary_key: PrimaryKey::new(["todo_id"]),
                version_column: Some("_sourced_version".into()),
                foreign_keys: Vec::new(),
                indexes: Vec::new(),
                relationships: Vec::new(),
                kind: TableKind::ReadModel,
            })
        }

        fn primary_key(&self) -> Result<RowKey, crate::table::TableStoreError> {
            Ok(RowKey::new([("todo_id", RowValue::U64(self.id))]))
        }

        fn to_row(&self) -> Result<RowValues, crate::table::TableStoreError> {
            let mut values = RowValues::new();
            values.insert("todo_id", RowValue::U64(self.id));
            values.insert("title", RowValue::String(self.title.clone()));
            Ok(values)
        }

        fn from_row(_row: RowValues) -> Result<Self, crate::table::TableStoreError> {
            unreachable!("workspace staging test does not hydrate rows")
        }
    }

    fn workspace() -> ProjectionWorkspace {
        let topology = ProjectorTopologyId::new(1, "project_todos", [4; 32]).unwrap();
        let mut codec = ProjectionScopeCodec::new(topology.clone());
        codec
            .register_model("TodoView", TodoView::schema())
            .unwrap();
        let partition_value = Some(serde_json::json!({"tenant": "a"}));
        let partition = codec.encode_partition(partition_value.as_ref()).unwrap();
        let cursor = ProjectionInputCursor::new(
            topology,
            partition,
            ProjectionSource::new("todo-stream", b"todo:7".to_vec()).unwrap(),
            ProjectionEpoch::new("source-v1").unwrap(),
            0,
        )
        .unwrap();
        let input = TrustedProjectionInput::mint(
            cursor,
            ProjectionInputFingerprint::from_canonical_bytes(b"input"),
            "message-1",
            "cause-1",
            ProjectionGeneration::initial(),
            true,
        )
        .unwrap();
        ProjectionWorkspace::new(
            Arc::new(codec),
            partition_value,
            input,
            ProjectionEpoch::new("changes-v1").unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn create_stages_one_sealed_mutation_and_framework_observation() {
        let mut workspace = workspace();
        workspace
            .create(&TodoView {
                id: 7,
                title: "write defensible code".into(),
            })
            .unwrap();
        let batch = workspace.into_batch().unwrap();
        assert_eq!(batch.mutations.len(), 1);
        assert_eq!(batch.observations.len(), 1);
        assert_eq!(batch.ownership.len(), 1);
        assert!(matches!(
            batch.observations[0].target,
            ProjectionObservationTarget::StagedRecord(_)
        ));
    }

    #[test]
    fn workspace_rejects_revision_from_another_canonical_scope() {
        let mut workspace = workspace();
        let wrong_scope = ProjectionRecordScope::new(
            workspace.input.cursor.topology().clone(),
            workspace.input.cursor.projection_partition().clone(),
            "TodoView",
            b"different-key".to_vec(),
        )
        .unwrap();
        let wrong_revision = RecordRevision::new(wrong_scope, 1, 1).unwrap();
        let error = workspace
            .save(
                &TodoView {
                    id: 7,
                    title: "changed".into(),
                },
                &wrong_revision,
            )
            .unwrap_err();
        assert!(matches!(
            error,
            ProjectionProtocolError::ScopeMismatch { .. }
        ));
    }
}
