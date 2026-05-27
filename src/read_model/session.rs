use std::collections::{BTreeMap, BTreeSet};
use std::marker::PhantomData;

use serde::Serialize;

use crate::repository::AsyncReadModelWritePlanStore;

use super::{
    ReadModel, ReadModelError, ReadModelSchema, RelationalReadModel, RelationalReadModelIncludes,
    RelationshipDef, RelationshipKind, RowKey, RowValue, RowValues, Versioned,
};

/// Expected optimistic version carried by a staged read-model write.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ExpectedVersion {
    /// No optimistic version check is requested.
    #[default]
    Any,
    /// The target row must currently have this version.
    Exact(u64),
    /// The target row must not exist yet.
    NotExists,
}

/// Full-row write behavior for a relational row mutation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowWriteMode {
    Insert,
    Upsert,
}

/// Sparse patch behavior for a relational row mutation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PatchMode {
    UpdateExisting,
    InsertMissing,
}

/// Adapter capabilities used to validate a write plan before any storage write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadModelAdapterCapabilities {
    pub relational_rows: bool,
    pub document_rows: bool,
    pub sparse_patches: bool,
    pub deletes: bool,
    pub processed_messages: bool,
}

impl Default for ReadModelAdapterCapabilities {
    fn default() -> Self {
        Self {
            relational_rows: true,
            document_rows: true,
            sparse_patches: true,
            deletes: true,
            processed_messages: true,
        }
    }
}

/// Result of applying a standalone read-model write plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadModelCommitOutcome {
    applied: bool,
    duplicate_message: Option<ProcessedMessageMark>,
}

impl ReadModelCommitOutcome {
    pub fn applied() -> Self {
        Self {
            applied: true,
            duplicate_message: None,
        }
    }

    pub fn skipped_duplicate(mark: ProcessedMessageMark) -> Self {
        Self {
            applied: false,
            duplicate_message: Some(mark),
        }
    }

    pub fn was_applied(&self) -> bool {
        self.applied
    }

    pub fn was_skipped(&self) -> bool {
        !self.applied
    }

    pub fn duplicate_message(&self) -> Option<&ProcessedMessageMark> {
        self.duplicate_message.as_ref()
    }
}

/// Adapter contract for committing read-model write plans without an aggregate repository.
pub trait ReadModelWritePlanStore: Send + Sync {
    fn read_model_capabilities(&self) -> ReadModelAdapterCapabilities;

    fn commit_write_plan(
        &self,
        plan: ReadModelWritePlan,
    ) -> Result<ReadModelCommitOutcome, ReadModelError>;

    fn is_processed(&self, consumer_name: &str, message_id: &str) -> Result<bool, ReadModelError>;
}

/// A request an adapter can satisfy with a primary-key read plus explicit includes.
#[derive(Clone, Debug, PartialEq)]
pub struct ReadModelLoadRequest {
    pub schema: ReadModelSchema,
    pub key: RowKey,
    pub includes: Vec<String>,
}

impl ReadModelLoadRequest {
    pub fn validate_for_query_capabilities(
        &self,
        capabilities: &ReadModelQueryCapabilities,
    ) -> Result<(), ReadModelError> {
        if !self.includes.is_empty() && !capabilities.relationship_includes {
            return Err(ReadModelError::Metadata(
                "read-model adapter does not support relationship includes".into(),
            ));
        }

        Ok(())
    }
}

/// Adapter capabilities for primary-key relational read-model loads.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReadModelQueryCapabilities {
    pub relationship_includes: bool,
}

impl ReadModelQueryCapabilities {
    pub fn relationship_includes() -> Self {
        Self {
            relationship_includes: true,
        }
    }
}

/// Rows loaded for one requested relationship include.
#[derive(Clone, Debug, PartialEq)]
pub struct ReadModelIncludeRows {
    pub relationship: RelationshipDef,
    pub target_schema: ReadModelSchema,
    pub rows: Vec<Versioned<RowValues>>,
}

/// Untyped graph loaded by a relational read-model adapter.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReadModelLoadGraph {
    pub root: Option<Versioned<RowValues>>,
    pub includes: BTreeMap<String, ReadModelIncludeRows>,
}

/// Adapter contract for explicit primary-key read-model loads and includes.
pub trait RelationalReadModelQueryStore: Send + Sync {
    fn read_model_query_capabilities(&self) -> ReadModelQueryCapabilities;

    fn load_graph(
        &self,
        request: ReadModelLoadRequest,
    ) -> Result<ReadModelLoadGraph, ReadModelError>;
}

/// Sparse column updates for a relational row.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RowPatch {
    values: RowValues,
}

impl RowPatch {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(mut self, column: impl Into<String>, value: RowValue) -> Self {
        self.values.insert(column, value);
        self
    }

    pub fn set_serde<T: Serialize + ?Sized>(
        mut self,
        column: impl Into<String>,
        value: &T,
    ) -> Result<Self, ReadModelError> {
        self.values.insert_serde(column, value)?;
        Ok(self)
    }

    pub fn get(&self, column: &str) -> Option<&RowValue> {
        self.values.get(column)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &RowValue)> {
        self.values.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn into_values(self) -> RowValues {
        self.values
    }
}

/// Whole-document read-model write backed by a document column such as JSONB.
#[derive(Clone, Debug, PartialEq)]
pub struct DocumentMutation {
    pub collection: String,
    pub id: String,
    pub bytes: Vec<u8>,
}

impl DocumentMutation {
    pub fn key(&self) -> String {
        document_key(&self.collection, &self.id)
    }
}

/// Full relational row insert/upsert mutation.
#[derive(Clone, Debug, PartialEq)]
pub struct RowMutation {
    pub schema: ReadModelSchema,
    pub key: RowKey,
    pub values: RowValues,
    pub expected_version: ExpectedVersion,
    pub mode: RowWriteMode,
}

/// Sparse relational row patch mutation.
#[derive(Clone, Debug, PartialEq)]
pub struct PatchRowMutation {
    pub schema: ReadModelSchema,
    pub key: RowKey,
    pub patch: RowPatch,
    pub expected_version: ExpectedVersion,
    pub mode: PatchMode,
}

/// Relational row delete mutation.
#[derive(Clone, Debug, PartialEq)]
pub struct DeleteRowMutation {
    pub schema: ReadModelSchema,
    pub key: RowKey,
    pub expected_version: ExpectedVersion,
}

/// First-pass read-model write-plan mutation surface.
#[derive(Clone, Debug, PartialEq)]
pub enum ReadModelMutation {
    Document(DocumentMutation),
    UpsertRow(RowMutation),
    PatchRow(PatchRowMutation),
    DeleteRow(DeleteRowMutation),
}

impl ReadModelMutation {
    pub fn table_name(&self) -> Option<&str> {
        match self {
            ReadModelMutation::Document(mutation) => Some(mutation.collection.as_str()),
            ReadModelMutation::UpsertRow(mutation) => Some(mutation.schema.table_name.as_str()),
            ReadModelMutation::PatchRow(mutation) => Some(mutation.schema.table_name.as_str()),
            ReadModelMutation::DeleteRow(mutation) => Some(mutation.schema.table_name.as_str()),
        }
    }

    pub fn lock_key(&self) -> String {
        match self {
            ReadModelMutation::Document(mutation) => mutation.key(),
            ReadModelMutation::UpsertRow(mutation) => format!(
                "{}:{}",
                mutation.schema.table_name,
                key_fingerprint(&mutation.key)
            ),
            ReadModelMutation::PatchRow(mutation) => format!(
                "{}:{}",
                mutation.schema.table_name,
                key_fingerprint(&mutation.key)
            ),
            ReadModelMutation::DeleteRow(mutation) => format!(
                "{}:{}",
                mutation.schema.table_name,
                key_fingerprint(&mutation.key)
            ),
        }
    }

    fn sort_key(&self) -> String {
        match self {
            ReadModelMutation::Document(mutation) => format!("0|{}", mutation.key()),
            ReadModelMutation::UpsertRow(mutation) => format!(
                "1|{}|{}",
                mutation.schema.table_name,
                key_fingerprint(&mutation.key)
            ),
            ReadModelMutation::PatchRow(mutation) => format!(
                "2|{}|{}",
                mutation.schema.table_name,
                key_fingerprint(&mutation.key)
            ),
            ReadModelMutation::DeleteRow(mutation) => format!(
                "3|{}|{}",
                mutation.schema.table_name,
                key_fingerprint(&mutation.key)
            ),
        }
    }
}

/// A processed-message marker staged with read-model writes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessedMessageMark {
    pub consumer_name: String,
    pub message_id: String,
}

/// Deterministic unit-of-work output for relational read-model adapters.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReadModelWritePlan {
    pub mutations: Vec<ReadModelMutation>,
    pub processed_messages: Vec<ProcessedMessageMark>,
}

impl ReadModelWritePlan {
    pub fn new(
        mutations: Vec<ReadModelMutation>,
        processed_messages: Vec<ProcessedMessageMark>,
    ) -> Self {
        Self {
            mutations,
            processed_messages,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.mutations.is_empty() && self.processed_messages.is_empty()
    }

    pub fn validate(&self) -> Result<(), ReadModelError> {
        self.validate_for(&ReadModelAdapterCapabilities::default())
    }

    pub fn validate_for(
        &self,
        capabilities: &ReadModelAdapterCapabilities,
    ) -> Result<(), ReadModelError> {
        for mutation in &self.mutations {
            match mutation {
                ReadModelMutation::Document(mutation) => {
                    if !capabilities.document_rows {
                        return Err(ReadModelError::Metadata(
                            "read-model adapter does not support document row writes".into(),
                        ));
                    }
                    validate_document_mutation(mutation)?;
                }
                ReadModelMutation::UpsertRow(mutation) => {
                    if !capabilities.relational_rows {
                        return Err(ReadModelError::Metadata(
                            "read-model adapter does not support relational row writes".into(),
                        ));
                    }
                    validate_row_mutation(mutation)?;
                }
                ReadModelMutation::PatchRow(mutation) => {
                    if !capabilities.relational_rows || !capabilities.sparse_patches {
                        return Err(ReadModelError::Metadata(
                            "read-model adapter does not support sparse row patches".into(),
                        ));
                    }
                    validate_patch_mutation(mutation)?;
                }
                ReadModelMutation::DeleteRow(mutation) => {
                    if !capabilities.relational_rows || !capabilities.deletes {
                        return Err(ReadModelError::Metadata(
                            "read-model adapter does not support row deletes".into(),
                        ));
                    }
                    validate_delete_mutation(mutation)?;
                }
            }
        }

        if !capabilities.processed_messages && !self.processed_messages.is_empty() {
            return Err(ReadModelError::Metadata(
                "read-model adapter does not support processed-message marks".into(),
            ));
        }

        for mark in &self.processed_messages {
            if mark.consumer_name.is_empty() || mark.message_id.is_empty() {
                return Err(ReadModelError::Metadata(
                    "processed-message marks require consumer name and message id".into(),
                ));
            }
        }

        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct RowIdentity {
    table_name: String,
    key: String,
}

#[derive(Clone, Debug)]
struct StagedMutation {
    sequence: u64,
    mutation: ReadModelMutation,
}

/// Detached builder for read-model write plans that are applied at commit.
#[derive(Clone, Debug, Default)]
pub struct ReadModelWritePlanBuilder {
    mutations: Vec<StagedMutation>,
    processed_messages: Vec<ProcessedMessageMark>,
    expected_versions: BTreeMap<RowIdentity, u64>,
    next_sequence: u64,
}

impl ReadModelWritePlanBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.mutations.is_empty() && self.processed_messages.is_empty()
    }

    pub fn load<M>(&self, key: RowKey) -> Result<ReadModelLoadRequest, ReadModelError>
    where
        M: RelationalReadModel,
    {
        self.load_with::<M, Vec<String>, String>(key, Vec::new())
    }

    pub fn load_with<M, I, S>(
        &self,
        key: RowKey,
        includes: I,
    ) -> Result<ReadModelLoadRequest, ReadModelError>
    where
        M: RelationalReadModel,
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let schema = validated_schema::<M>()?;
        validate_key(&schema, &key)?;
        let includes: Vec<String> = includes.into_iter().map(Into::into).collect();
        for include in &includes {
            if !schema
                .relationships
                .iter()
                .any(|relationship| relationship.field_name == *include)
            {
                return Err(ReadModelError::Metadata(format!(
                    "read model `{}` has no relationship `{}`",
                    schema.model_name, include
                )));
            }
        }

        Ok(ReadModelLoadRequest {
            schema,
            key,
            includes,
        })
    }

    pub fn track_loaded<M>(&mut self, versioned: &Versioned<M>) -> Result<&mut Self, ReadModelError>
    where
        M: RelationalReadModel,
    {
        self.expect_version::<M>(versioned.data.primary_key()?, versioned.version)
    }

    pub fn expect_version<M>(
        &mut self,
        key: RowKey,
        expected_version: u64,
    ) -> Result<&mut Self, ReadModelError>
    where
        M: RelationalReadModel,
    {
        let schema = validated_schema::<M>()?;
        validate_key(&schema, &key)?;
        validate_expected_version(&ExpectedVersion::Exact(expected_version), &schema)?;
        self.expected_versions.insert(
            RowIdentity {
                table_name: schema.table_name,
                key: key_fingerprint(&key),
            },
            expected_version,
        );
        Ok(self)
    }

    pub fn insert<M>(&mut self, model: &M) -> Result<&mut Self, ReadModelError>
    where
        M: RelationalReadModel,
    {
        self.stage_full_row(
            model,
            RowWriteMode::Insert,
            Some(ExpectedVersion::NotExists),
        )
    }

    pub fn upsert<M>(&mut self, model: &M) -> Result<&mut Self, ReadModelError>
    where
        M: RelationalReadModel,
    {
        self.stage_full_row(model, RowWriteMode::Upsert, None)
    }

    pub fn insert_related<P, C>(
        &mut self,
        parent: &P,
        relationship_field: &str,
        child: &C,
    ) -> Result<&mut Self, ReadModelError>
    where
        P: RelationalReadModel,
        C: RelationalReadModel,
    {
        self.stage_related_row(parent, relationship_field, child, RowWriteMode::Insert)
    }

    pub fn upsert_related<P, C>(
        &mut self,
        parent: &P,
        relationship_field: &str,
        child: &C,
    ) -> Result<&mut Self, ReadModelError>
    where
        P: RelationalReadModel,
        C: RelationalReadModel,
    {
        self.stage_related_row(parent, relationship_field, child, RowWriteMode::Upsert)
    }

    pub fn patch<M>(&mut self, key: RowKey, patch: RowPatch) -> Result<&mut Self, ReadModelError>
    where
        M: RelationalReadModel,
    {
        self.stage_patch::<M>(key, patch, PatchMode::UpdateExisting)
    }

    pub fn upsert_patch<M>(
        &mut self,
        key: RowKey,
        patch: RowPatch,
    ) -> Result<&mut Self, ReadModelError>
    where
        M: RelationalReadModel,
    {
        self.stage_patch::<M>(key, patch, PatchMode::InsertMissing)
    }

    pub fn delete<M>(&mut self, key: RowKey) -> Result<&mut Self, ReadModelError>
    where
        M: RelationalReadModel,
    {
        let schema = validated_schema::<M>()?;
        validate_key(&schema, &key)?;
        let expected_version = self.expected_for(&schema, &key);
        let mutation = DeleteRowMutation {
            schema,
            key,
            expected_version,
        };
        self.push(ReadModelMutation::DeleteRow(mutation));
        Ok(self)
    }

    pub fn delete_model<M>(&mut self, model: &M) -> Result<&mut Self, ReadModelError>
    where
        M: RelationalReadModel,
    {
        self.delete::<M>(model.primary_key()?)
    }

    pub fn document<M>(&mut self, model: &M) -> Result<&mut Self, ReadModelError>
    where
        M: ReadModel,
    {
        let bytes =
            serde_json::to_vec(model).map_err(|err| ReadModelError::Serde(err.to_string()))?;
        self.push(ReadModelMutation::Document(DocumentMutation {
            collection: M::COLLECTION.to_string(),
            id: model.id().to_string(),
            bytes,
        }));
        Ok(self)
    }

    pub fn mark_processed(
        &mut self,
        consumer_name: impl Into<String>,
        message_id: impl Into<String>,
    ) -> &mut Self {
        self.processed_messages.push(ProcessedMessageMark {
            consumer_name: consumer_name.into(),
            message_id: message_id.into(),
        });
        self
    }

    pub fn into_write_plan(self) -> Result<ReadModelWritePlan, ReadModelError> {
        let mut mutations = self.mutations;
        mutations.sort_by(|left, right| {
            left.mutation
                .sort_key()
                .cmp(&right.mutation.sort_key())
                .then(left.sequence.cmp(&right.sequence))
        });
        let mutations = mutations
            .into_iter()
            .map(|staged| staged.mutation)
            .collect::<Vec<_>>();
        let plan = ReadModelWritePlan::new(mutations, self.processed_messages);
        plan.validate()?;
        Ok(plan)
    }

    pub fn commit<S>(self, store: &S) -> Result<ReadModelCommitOutcome, ReadModelError>
    where
        S: ReadModelWritePlanStore + ?Sized,
    {
        store.commit_write_plan(self.into_write_plan()?)
    }

    pub async fn commit_async<S>(self, store: &S) -> Result<ReadModelCommitOutcome, ReadModelError>
    where
        S: AsyncReadModelWritePlanStore + ?Sized,
    {
        store.commit_write_plan_async(self.into_write_plan()?).await
    }

    fn stage_full_row<M>(
        &mut self,
        model: &M,
        mode: RowWriteMode,
        expected_version: Option<ExpectedVersion>,
    ) -> Result<&mut Self, ReadModelError>
    where
        M: RelationalReadModel,
    {
        let schema = validated_schema::<M>()?;
        let key = model.primary_key()?;
        let values = model.to_row()?;
        validate_key(&schema, &key)?;
        let expected_version = expected_version.unwrap_or_else(|| self.expected_for(&schema, &key));
        let mutation = RowMutation {
            schema,
            key,
            values,
            expected_version,
            mode,
        };
        self.push(ReadModelMutation::UpsertRow(mutation));
        Ok(self)
    }

    fn stage_related_row<P, C>(
        &mut self,
        parent: &P,
        relationship_field: &str,
        child: &C,
        mode: RowWriteMode,
    ) -> Result<&mut Self, ReadModelError>
    where
        P: RelationalReadModel,
        C: RelationalReadModel,
    {
        let parent_schema = validated_schema::<P>()?;
        let child_schema = validated_schema::<C>()?;
        let relationship = parent_schema
            .relationships
            .iter()
            .find(|relationship| relationship.field_name == relationship_field)
            .ok_or_else(|| {
                ReadModelError::Metadata(format!(
                    "read model `{}` has no relationship `{}`",
                    parent_schema.model_name, relationship_field
                ))
            })?;

        if relationship.target_model != child_schema.model_name {
            return Err(ReadModelError::Metadata(format!(
                "relationship `{}` targets `{}`, not `{}`",
                relationship.field_name, relationship.target_model, child_schema.model_name
            )));
        }

        let parent_row = parent.to_row()?;
        let mut child_row = child.to_row()?;
        populate_delegated_relationship_values(
            &parent_schema,
            &parent_row,
            relationship,
            &child_schema,
            &mut child_row,
        )?;
        let key = key_from_row(&child_schema, &child_row)?;
        let expected_version = match mode {
            RowWriteMode::Insert => ExpectedVersion::NotExists,
            RowWriteMode::Upsert => self.expected_for(&child_schema, &key),
        };
        let mutation = RowMutation {
            schema: child_schema,
            key,
            values: child_row,
            expected_version,
            mode,
        };
        self.push(ReadModelMutation::UpsertRow(mutation));
        Ok(self)
    }

    fn stage_patch<M>(
        &mut self,
        key: RowKey,
        patch: RowPatch,
        mode: PatchMode,
    ) -> Result<&mut Self, ReadModelError>
    where
        M: RelationalReadModel,
    {
        let schema = validated_schema::<M>()?;
        validate_key(&schema, &key)?;
        let expected_version = self.expected_for(&schema, &key);
        let mutation = PatchRowMutation {
            schema,
            key,
            patch,
            expected_version,
            mode,
        };
        self.push(ReadModelMutation::PatchRow(mutation));
        Ok(self)
    }

    fn push(&mut self, mutation: ReadModelMutation) {
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.saturating_add(1);
        self.mutations.push(StagedMutation { sequence, mutation });
    }

    fn expected_for(&self, schema: &ReadModelSchema, key: &RowKey) -> ExpectedVersion {
        self.expected_versions
            .get(&RowIdentity {
                table_name: schema.table_name.clone(),
                key: key_fingerprint(key),
            })
            .copied()
            .map(ExpectedVersion::Exact)
            .unwrap_or(ExpectedVersion::Any)
    }
}

#[derive(Clone, Debug)]
struct TrackedRowBaseline {
    key: RowKey,
    row: RowValues,
    version: u64,
}

#[derive(Clone, Debug)]
struct TrackedIncludeBaseline {
    relationship: RelationshipDef,
    target_schema: ReadModelSchema,
    rows: BTreeMap<String, TrackedRowBaseline>,
}

#[derive(Clone, Debug)]
struct TrackedModelBaseline {
    root_schema: ReadModelSchema,
    root_key: RowKey,
    root_row: RowValues,
    root_version: u64,
    includes: BTreeMap<String, TrackedIncludeBaseline>,
}

/// Store-bound read-model workspace for load, mutate, sync, commit workflows.
pub struct ReadModelWorkspace<'a, S> {
    store: &'a S,
    writes: ReadModelWritePlanBuilder,
    baselines: Vec<TrackedModelBaseline>,
}

impl<'a, S> ReadModelWorkspace<'a, S>
where
    S: ReadModelWritePlanStore + RelationalReadModelQueryStore,
{
    pub fn new(store: &'a S) -> Self {
        Self {
            store,
            writes: ReadModelWritePlanBuilder::new(),
            baselines: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.writes.is_empty()
    }

    pub fn load<M>(&mut self, key: RowKey) -> ReadModelLoadBuilder<'_, 'a, S, M>
    where
        M: RelationalReadModel + RelationalReadModelIncludes,
    {
        ReadModelLoadBuilder {
            unit: self,
            key,
            includes: Vec::new(),
            _marker: PhantomData,
        }
    }

    pub fn sync<M>(&mut self, model: M) -> Result<&mut Self, ReadModelError>
    where
        M: RelationalReadModel + RelationalReadModelIncludes,
    {
        let schema = validated_schema::<M>()?;
        let key = model.primary_key()?;
        validate_key(&schema, &key)?;
        let identity = RowIdentity {
            table_name: schema.table_name.clone(),
            key: key_fingerprint(&key),
        };
        let baseline = self
            .baselines
            .iter()
            .find(|baseline| {
                baseline.root_schema.table_name == identity.table_name
                    && key_fingerprint(&baseline.root_key) == identity.key
            })
            .cloned()
            .ok_or_else(|| {
                ReadModelError::Metadata(format!(
                    "read model `{}` has no tracked baseline for sync",
                    schema.model_name
                ))
            })?;
        let current_row = model.to_row()?;

        self.stage_row_diff(
            schema,
            key,
            &baseline.root_row,
            &current_row,
            baseline.root_version,
        )?;

        for (include_name, include) in &baseline.includes {
            let current_rows = model.include_rows(include_name)?;
            self.stage_include_changes(&baseline.root_schema, &current_row, include, current_rows)?;
        }

        Ok(self)
    }

    pub fn upsert<M>(&mut self, model: &M) -> Result<&mut Self, ReadModelError>
    where
        M: RelationalReadModel,
    {
        self.writes.upsert(model)?;
        Ok(self)
    }

    pub fn insert<M>(&mut self, model: &M) -> Result<&mut Self, ReadModelError>
    where
        M: RelationalReadModel,
    {
        self.writes.insert(model)?;
        Ok(self)
    }

    pub fn upsert_related<P, C>(
        &mut self,
        parent: &P,
        relationship_field: &str,
        child: &C,
    ) -> Result<&mut Self, ReadModelError>
    where
        P: RelationalReadModel,
        C: RelationalReadModel,
    {
        self.writes
            .upsert_related(parent, relationship_field, child)?;
        Ok(self)
    }

    pub fn insert_related<P, C>(
        &mut self,
        parent: &P,
        relationship_field: &str,
        child: &C,
    ) -> Result<&mut Self, ReadModelError>
    where
        P: RelationalReadModel,
        C: RelationalReadModel,
    {
        self.writes
            .insert_related(parent, relationship_field, child)?;
        Ok(self)
    }

    pub fn patch<M>(&mut self, key: RowKey, patch: RowPatch) -> Result<&mut Self, ReadModelError>
    where
        M: RelationalReadModel,
    {
        self.writes.patch::<M>(key, patch)?;
        Ok(self)
    }

    pub fn upsert_patch<M>(
        &mut self,
        key: RowKey,
        patch: RowPatch,
    ) -> Result<&mut Self, ReadModelError>
    where
        M: RelationalReadModel,
    {
        self.writes.upsert_patch::<M>(key, patch)?;
        Ok(self)
    }

    pub fn delete<M>(&mut self, key: RowKey) -> Result<&mut Self, ReadModelError>
    where
        M: RelationalReadModel,
    {
        self.writes.delete::<M>(key)?;
        Ok(self)
    }

    pub fn delete_model<M>(&mut self, model: &M) -> Result<&mut Self, ReadModelError>
    where
        M: RelationalReadModel,
    {
        self.writes.delete_model(model)?;
        Ok(self)
    }

    pub fn document<M>(&mut self, model: &M) -> Result<&mut Self, ReadModelError>
    where
        M: ReadModel,
    {
        self.writes.document(model)?;
        Ok(self)
    }

    pub fn mark_processed(
        &mut self,
        consumer_name: impl Into<String>,
        message_id: impl Into<String>,
    ) -> &mut Self {
        self.writes.mark_processed(consumer_name, message_id);
        self
    }

    pub fn into_write_plan(self) -> Result<ReadModelWritePlan, ReadModelError> {
        self.writes.into_write_plan()
    }

    pub fn commit(self) -> Result<ReadModelCommitOutcome, ReadModelError> {
        self.writes.commit(self.store)
    }

    fn track_graph(
        &mut self,
        schema: ReadModelSchema,
        root: Versioned<RowValues>,
        includes: BTreeMap<String, ReadModelIncludeRows>,
    ) -> Result<(), ReadModelError> {
        let root_key = key_from_row(&schema, &root.data)?;
        let root_identity = RowIdentity {
            table_name: schema.table_name.clone(),
            key: key_fingerprint(&root_key),
        };
        self.writes
            .expected_versions
            .insert(root_identity, root.version);

        let mut tracked_includes = BTreeMap::new();
        for (include_name, include_rows) in includes {
            let mut rows = BTreeMap::new();
            for row in include_rows.rows {
                let key = key_from_row(&include_rows.target_schema, &row.data)?;
                rows.insert(
                    key_fingerprint(&key),
                    TrackedRowBaseline {
                        key,
                        row: row.data,
                        version: row.version,
                    },
                );
            }
            tracked_includes.insert(
                include_name,
                TrackedIncludeBaseline {
                    relationship: include_rows.relationship,
                    target_schema: include_rows.target_schema,
                    rows,
                },
            );
        }

        let fingerprint = key_fingerprint(&root_key);
        self.baselines.retain(|baseline| {
            baseline.root_schema.table_name != schema.table_name
                || key_fingerprint(&baseline.root_key) != fingerprint
        });
        self.baselines.push(TrackedModelBaseline {
            root_schema: schema,
            root_key,
            root_row: root.data,
            root_version: root.version,
            includes: tracked_includes,
        });
        Ok(())
    }

    fn stage_include_changes(
        &mut self,
        root_schema: &ReadModelSchema,
        root_row: &RowValues,
        baseline: &TrackedIncludeBaseline,
        current_rows: Vec<RowValues>,
    ) -> Result<(), ReadModelError> {
        if matches!(baseline.relationship.kind, RelationshipKind::BelongsTo)
            && current_rows.len() > 1
        {
            return Err(ReadModelError::Metadata(format!(
                "belongs_to relationship `{}` can sync at most one related row",
                baseline.relationship.field_name
            )));
        }

        let mut current_fingerprints = BTreeSet::new();
        for mut current_row in current_rows {
            match baseline.relationship.kind {
                RelationshipKind::HasMany => populate_delegated_relationship_values(
                    root_schema,
                    root_row,
                    &baseline.relationship,
                    &baseline.target_schema,
                    &mut current_row,
                )?,
                RelationshipKind::BelongsTo => {}
                RelationshipKind::ManyToMany => {
                    return Err(ReadModelError::Metadata(format!(
                        "many-to-many relationship `{}` includes are not supported yet",
                        baseline.relationship.field_name
                    )));
                }
            }

            let key = key_from_row(&baseline.target_schema, &current_row)?;
            let fingerprint = key_fingerprint(&key);
            current_fingerprints.insert(fingerprint.clone());
            if let Some(loaded) = baseline.rows.get(&fingerprint) {
                self.stage_row_diff(
                    baseline.target_schema.clone(),
                    loaded.key.clone(),
                    &loaded.row,
                    &current_row,
                    loaded.version,
                )?;
            } else {
                self.stage_upsert_row(baseline.target_schema.clone(), key, current_row)?;
            }
        }

        // `sync` makes storage match the struct: an owned `has_many` child
        // dropped from the loaded collection is deleted. `belongs_to` clears never
        // delete the target, which is the owner that other rows may reference.
        if matches!(baseline.relationship.kind, RelationshipKind::HasMany) {
            for (fingerprint, loaded) in &baseline.rows {
                if !current_fingerprints.contains(fingerprint) {
                    self.stage_delete_row(
                        baseline.target_schema.clone(),
                        loaded.key.clone(),
                        loaded.version,
                    )?;
                }
            }
        }

        Ok(())
    }

    fn stage_row_diff(
        &mut self,
        schema: ReadModelSchema,
        key: RowKey,
        before: &RowValues,
        after: &RowValues,
        expected_version: u64,
    ) -> Result<(), ReadModelError> {
        let patch = diff_rows(before, after);
        if patch.is_empty() {
            return Ok(());
        }

        let mutation = PatchRowMutation {
            schema,
            key,
            patch,
            expected_version: ExpectedVersion::Exact(expected_version),
            mode: PatchMode::UpdateExisting,
        };
        validate_patch_mutation(&mutation)?;
        self.writes.push(ReadModelMutation::PatchRow(mutation));
        Ok(())
    }

    fn stage_upsert_row(
        &mut self,
        schema: ReadModelSchema,
        key: RowKey,
        values: RowValues,
    ) -> Result<(), ReadModelError> {
        let mutation = RowMutation {
            schema,
            key,
            values,
            expected_version: ExpectedVersion::Any,
            mode: RowWriteMode::Upsert,
        };
        validate_row_mutation(&mutation)?;
        self.writes.push(ReadModelMutation::UpsertRow(mutation));
        Ok(())
    }

    fn stage_delete_row(
        &mut self,
        schema: ReadModelSchema,
        key: RowKey,
        expected_version: u64,
    ) -> Result<(), ReadModelError> {
        let mutation = DeleteRowMutation {
            schema,
            key,
            expected_version: ExpectedVersion::Exact(expected_version),
        };
        validate_delete_mutation(&mutation)?;
        self.writes.push(ReadModelMutation::DeleteRow(mutation));
        Ok(())
    }
}

/// Builder for one explicit primary-key read-model load.
pub struct ReadModelLoadBuilder<'workspace, 'store, S, M>
where
    S: ReadModelWritePlanStore + RelationalReadModelQueryStore,
{
    unit: &'workspace mut ReadModelWorkspace<'store, S>,
    key: RowKey,
    includes: Vec<String>,
    _marker: PhantomData<M>,
}

impl<'workspace, 'store, S, M> ReadModelLoadBuilder<'workspace, 'store, S, M>
where
    S: ReadModelWritePlanStore + RelationalReadModelQueryStore,
    M: RelationalReadModel + RelationalReadModelIncludes,
{
    pub fn include(mut self, relationship: impl Into<String>) -> Self {
        self.includes.push(relationship.into());
        self
    }

    pub fn one(self) -> Result<Option<Versioned<M>>, ReadModelError> {
        let request = self
            .unit
            .writes
            .load_with::<M, _, _>(self.key, self.includes)?;
        let graph = self.unit.store.load_graph(request.clone())?;
        let Some(root) = graph.root else {
            return Ok(None);
        };

        let mut model = M::from_row(root.data.clone())?;
        for (include_name, include_rows) in &graph.includes {
            let rows = include_rows
                .rows
                .iter()
                .map(|row| row.data.clone())
                .collect::<Vec<_>>();
            model.hydrate_include(include_name, rows)?;
        }

        self.unit
            .track_graph(request.schema, root.clone(), graph.includes)?;
        Ok(Some(Versioned {
            data: model,
            version: root.version,
        }))
    }
}

/// Extension trait that starts a tracked read-model workspace from a store.
pub trait ReadModelWorkspaceExt:
    ReadModelWritePlanStore + RelationalReadModelQueryStore + Sized
{
    fn workspace(&self) -> ReadModelWorkspace<'_, Self> {
        ReadModelWorkspace::new(self)
    }
}

impl<S> ReadModelWorkspaceExt for S where S: ReadModelWritePlanStore + RelationalReadModelQueryStore {}

fn diff_rows(before: &RowValues, after: &RowValues) -> RowPatch {
    let mut patch = RowPatch::new();
    for (column, value) in after.iter() {
        if before.get(column) != Some(value) {
            patch = patch.set(column.to_string(), value.clone());
        }
    }
    patch
}

fn validated_schema<M>() -> Result<ReadModelSchema, ReadModelError>
where
    M: RelationalReadModel,
{
    let schema = M::schema();
    schema.validate()?;
    Ok(schema)
}

fn validate_document_mutation(mutation: &DocumentMutation) -> Result<(), ReadModelError> {
    if mutation.collection.is_empty() || mutation.id.is_empty() {
        return Err(ReadModelError::Metadata(
            "document read-model writes require collection and id".into(),
        ));
    }
    Ok(())
}

fn validate_row_mutation(mutation: &RowMutation) -> Result<(), ReadModelError> {
    mutation.schema.validate()?;
    validate_key(&mutation.schema, &mutation.key)?;
    validate_expected_version(&mutation.expected_version, &mutation.schema)?;
    validate_row_values(&mutation.schema, &mutation.values, true)
}

fn validate_patch_mutation(mutation: &PatchRowMutation) -> Result<(), ReadModelError> {
    mutation.schema.validate()?;
    validate_key(&mutation.schema, &mutation.key)?;
    validate_expected_version(&mutation.expected_version, &mutation.schema)?;
    if mutation.patch.is_empty() {
        return Err(ReadModelError::Metadata(format!(
            "read model `{}` patch must set at least one column",
            mutation.schema.model_name
        )));
    }
    validate_row_values(&mutation.schema, &mutation.patch.values, false)
}

fn validate_delete_mutation(mutation: &DeleteRowMutation) -> Result<(), ReadModelError> {
    mutation.schema.validate()?;
    validate_key(&mutation.schema, &mutation.key)?;
    validate_expected_version(&mutation.expected_version, &mutation.schema)
}

fn validate_expected_version(
    expected_version: &ExpectedVersion,
    schema: &ReadModelSchema,
) -> Result<(), ReadModelError> {
    if matches!(expected_version, ExpectedVersion::Exact(0)) {
        return Err(ReadModelError::Metadata(format!(
            "read model `{}` expected version must be greater than zero",
            schema.model_name
        )));
    }
    Ok(())
}

pub(crate) fn validate_key(schema: &ReadModelSchema, key: &RowKey) -> Result<(), ReadModelError> {
    if key.is_empty() {
        return Err(ReadModelError::Metadata(format!(
            "read model `{}` row key cannot be empty",
            schema.model_name
        )));
    }

    for column in &schema.primary_key.columns {
        match key.get(column) {
            Some(RowValue::Null) => {
                return Err(ReadModelError::Metadata(format!(
                    "read model `{}` primary-key column `{}` cannot be null",
                    schema.model_name, column
                )));
            }
            Some(_) => {}
            None => {
                return Err(ReadModelError::Metadata(format!(
                    "read model `{}` row key is missing primary-key column `{}`",
                    schema.model_name, column
                )));
            }
        }
    }

    for (column, _) in key.iter() {
        if !schema.primary_key.columns.iter().any(|key| key == column) {
            return Err(ReadModelError::Metadata(format!(
                "read model `{}` row key includes non-primary-key column `{}`",
                schema.model_name, column
            )));
        }
    }

    Ok(())
}

pub(crate) fn validate_row_values(
    schema: &ReadModelSchema,
    values: &RowValues,
    full_row: bool,
) -> Result<(), ReadModelError> {
    for (column_name, value) in values.iter() {
        let column = schema
            .columns
            .iter()
            .find(|column| column.column_name == column_name)
            .ok_or_else(|| {
                ReadModelError::Metadata(format!(
                    "read model `{}` write references missing column `{}`",
                    schema.model_name, column_name
                ))
            })?;

        if matches!(value, RowValue::Null) {
            if column.primary_key {
                return Err(ReadModelError::Metadata(format!(
                    "read model `{}` primary-key column `{}` cannot be null",
                    schema.model_name, column.column_name
                )));
            }
            if !column.nullable && !column.has_default {
                return Err(ReadModelError::Metadata(format!(
                    "read model `{}` column `{}` is not nullable",
                    schema.model_name, column.column_name
                )));
            }
        }
    }

    if full_row {
        for column in &schema.columns {
            if column.skipped || column.nullable || column.has_default {
                continue;
            }
            if !values.contains_key(&column.column_name) {
                return Err(ReadModelError::Metadata(format!(
                    "read model `{}` row is missing required column `{}`",
                    schema.model_name, column.column_name
                )));
            }
        }

        for column in schema
            .columns
            .iter()
            .filter(|column| column.delegated_from.is_some())
        {
            match values.get(&column.column_name) {
                Some(RowValue::Null) | None => {
                    return Err(ReadModelError::Metadata(format!(
                        "read model `{}` delegated column `{}` must be populated before write",
                        schema.model_name, column.column_name
                    )));
                }
                Some(_) => {}
            }
        }
    }

    Ok(())
}

pub(crate) fn key_from_row(
    schema: &ReadModelSchema,
    row: &RowValues,
) -> Result<RowKey, ReadModelError> {
    let mut key = RowKey::default();
    for column in &schema.primary_key.columns {
        let value = row.get(column).cloned().ok_or_else(|| {
            ReadModelError::Metadata(format!(
                "read model `{}` row is missing primary-key column `{}`",
                schema.model_name, column
            ))
        })?;
        key.insert(column.clone(), value);
    }
    validate_key(schema, &key)?;
    Ok(key)
}

fn populate_delegated_relationship_values(
    parent_schema: &ReadModelSchema,
    parent_row: &RowValues,
    relationship: &RelationshipDef,
    child_schema: &ReadModelSchema,
    child_row: &mut RowValues,
) -> Result<(), ReadModelError> {
    let mut populated = 0;
    for column in child_schema
        .columns
        .iter()
        .filter(|column| column.delegated_from.is_some())
    {
        let delegated_from = column.delegated_from.as_deref().unwrap_or_default();
        let Some((model_name, source_name)) = delegated_from.split_once('.') else {
            return Err(ReadModelError::Metadata(format!(
                "read model `{}` delegated column `{}` has invalid source `{}`",
                child_schema.model_name, column.column_name, delegated_from
            )));
        };

        if model_name != parent_schema.model_name {
            continue;
        }

        let source_column = column_name_for(parent_schema, source_name).ok_or_else(|| {
            ReadModelError::Metadata(format!(
                "read model `{}` delegated source `{}` is not a parent column",
                child_schema.model_name, delegated_from
            ))
        })?;
        let value = parent_row.get(&source_column).cloned().ok_or_else(|| {
            ReadModelError::Metadata(format!(
                "read model `{}` parent row is missing delegated source column `{}`",
                parent_schema.model_name, source_column
            ))
        })?;
        child_row.insert(column.column_name.clone(), value);
        populated += 1;
    }

    if populated == 0 {
        let foreign_key = relationship.foreign_key.as_deref().ok_or_else(|| {
            ReadModelError::Metadata(format!(
                "read model `{}` relationship `{}` must declare a foreign key",
                parent_schema.model_name, relationship.field_name
            ))
        })?;
        let child_column = column_name_for(child_schema, foreign_key).ok_or_else(|| {
            ReadModelError::Metadata(format!(
                "relationship `{}` foreign key `{}` is not a child column",
                relationship.field_name, foreign_key
            ))
        })?;
        let parent_column = column_name_for(parent_schema, foreign_key)
            .or_else(|| parent_schema.primary_key.columns.first().cloned())
            .ok_or_else(|| {
                ReadModelError::Metadata(format!(
                    "relationship `{}` has no parent key to delegate",
                    relationship.field_name
                ))
            })?;
        let value = parent_row.get(&parent_column).cloned().ok_or_else(|| {
            ReadModelError::Metadata(format!(
                "read model `{}` parent row is missing relationship key `{}`",
                parent_schema.model_name, parent_column
            ))
        })?;
        child_row.insert(child_column, value);
    }

    Ok(())
}

pub(crate) fn column_name_for(schema: &ReadModelSchema, field_or_column: &str) -> Option<String> {
    schema
        .columns
        .iter()
        .find(|column| {
            column.field_name == field_or_column || column.column_name == field_or_column
        })
        .map(|column| column.column_name.clone())
}

pub(crate) fn key_fingerprint(key: &RowKey) -> String {
    let mut fingerprint = String::new();
    for (column, value) in key.iter() {
        push_fingerprint_part(&mut fingerprint, column);
        push_fingerprint_part(&mut fingerprint, &value_fingerprint(value));
    }
    fingerprint
}

pub(crate) fn document_key(collection: &str, id: &str) -> String {
    let mut key = document_key_prefix(collection);
    push_fingerprint_part(&mut key, id);
    key
}

pub(crate) fn document_key_prefix(collection: &str) -> String {
    let mut prefix = String::new();
    push_fingerprint_part(&mut prefix, collection);
    prefix
}

fn push_fingerprint_part(fingerprint: &mut String, part: &str) {
    fingerprint.push_str(&part.len().to_string());
    fingerprint.push(':');
    fingerprint.push_str(part);
    fingerprint.push(';');
}

fn value_fingerprint(value: &RowValue) -> String {
    match value {
        RowValue::Null => "null".into(),
        RowValue::Bool(value) => format!("bool:{value}"),
        RowValue::I64(value) => format!("i64:{value}"),
        RowValue::U64(value) => format!("u64:{value}"),
        RowValue::F64(value) => format!("f64:{value:?}"),
        RowValue::String(value) => format!("string:{value}"),
        RowValue::Bytes(value) => format!("bytes:{value:?}"),
        RowValue::Json(value) => format!(
            "json:{}",
            serde_json::to_string(value).unwrap_or_else(|_| value.to_string())
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_fingerprint_distinguishes_delimiter_collisions() {
        let left = RowKey::new([
            ("a", RowValue::String("x,b=y".into())),
            ("b", RowValue::String("z".into())),
        ]);
        let right = RowKey::new([
            ("a", RowValue::String("x".into())),
            ("b", RowValue::String("y,b=z".into())),
        ]);

        assert_ne!(key_fingerprint(&left), key_fingerprint(&right));
    }

    #[test]
    fn key_fingerprint_distinguishes_row_value_types() {
        let integer = RowKey::new([("id", RowValue::I64(1))]);
        let string = RowKey::new([("id", RowValue::String("1".into()))]);

        assert_ne!(key_fingerprint(&integer), key_fingerprint(&string));
    }

    #[test]
    fn document_key_distinguishes_delimiter_collisions() {
        let left = DocumentMutation {
            collection: "a:b".into(),
            id: "c".into(),
            bytes: Vec::new(),
        };
        let right = DocumentMutation {
            collection: "a".into(),
            id: "b:c".into(),
            bytes: Vec::new(),
        };

        assert_ne!(left.key(), right.key());
    }
}
