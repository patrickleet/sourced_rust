//! Deterministic read-model write plans and the detached builder that stages them.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use crate::repository::ReadModelWritePlanStore;

use super::mutation::{
    column_name_for, key_fingerprint, key_from_row, validate_delete_mutation,
    validate_expected_version, validate_key, validate_patch_mutation, validate_row_mutation,
};
use super::{
    DeleteRowMutation, ExpectedVersion, PatchMode, PatchRowMutation, ReadModelAdapterCapabilities,
    ReadModelError, ReadModelLoadRequest, ReadModelMutation, ReadModelSchema, RelationalReadModel,
    RelationshipDef, RowKey, RowMutation, RowPatch, RowValues, RowWriteMode, Versioned,
};

/// Result of applying a standalone read-model write plan.
///
/// This is intentionally a stub: it carries no skipped/replay state and
/// [`was_applied`](Self::was_applied) is always `true`. The earlier
/// `read_model_processed_messages` dedupe table and `skipped_duplicate` outcome
/// were **deliberately removed** (see `specs/consumer-inbox-design.md`, decision
/// 2026-05-28) because coupling delivery-level dedupe to the read-model
/// projection contract was the wrong boundary. Replay safety is now a projection
/// convention — handlers make their writes idempotent so a redelivered event
/// re-converges (plus per-row `ExpectedVersion` optimistic concurrency). A
/// first-class replay barrier returns with the consumer inbox (an operational
/// `consumer_inbox` table committed as a `CommitBatch` participant), tracked
/// under `tasks/build-transport-bus-facade`; the variant set will grow then.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReadModelCommitOutcome;

impl ReadModelCommitOutcome {
    /// The write plan was applied. Currently the only outcome (see the type docs).
    pub fn applied() -> Self {
        Self
    }

    /// Always `true` today — see the type docs for why there is no skipped variant.
    pub fn was_applied(&self) -> bool {
        true
    }
}

/// Deterministic unit-of-work output for relational read-model adapters.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReadModelWritePlan {
    pub mutations: Vec<ReadModelMutation>,
}

impl ReadModelWritePlan {
    pub fn new(mutations: Vec<ReadModelMutation>) -> Self {
        Self { mutations }
    }

    pub fn is_empty(&self) -> bool {
        self.mutations.is_empty()
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

        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct RowIdentity {
    pub(super) table_name: String,
    pub(super) key: String,
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
    pub(super) expected_versions: BTreeMap<RowIdentity, u64>,
    next_sequence: u64,
}

impl ReadModelWritePlanBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.mutations.is_empty()
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
        validate_key(schema, &key)?;
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
            schema: schema.clone(),
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
        validate_key(schema, &key)?;
        validate_expected_version(&ExpectedVersion::Exact(expected_version), schema)?;
        self.expected_versions.insert(
            RowIdentity {
                table_name: schema.table_name.clone(),
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
        validate_key(schema, &key)?;
        let expected_version = self.expected_for(schema, &key);
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

    pub fn into_write_plan(self) -> Result<ReadModelWritePlan, ReadModelError> {
        // Precompute each mutation's sort key once; building the formatted key
        // inside the comparator would allocate two Strings per comparison.
        let mut mutations = self
            .mutations
            .into_iter()
            .map(|staged| (staged.mutation.sort_key(), staged))
            .collect::<Vec<_>>();
        mutations.sort_by(|(left_key, left), (right_key, right)| {
            left.mutation
                .operation_rank()
                .cmp(&right.mutation.operation_rank())
                .then_with(|| {
                    left.mutation
                        .dependency_order(&right.mutation)
                        .unwrap_or(Ordering::Equal)
                })
                .then_with(|| left_key.cmp(right_key))
                .then(left.sequence.cmp(&right.sequence))
        });
        let mutations = mutations
            .into_iter()
            .map(|(_, staged)| staged.mutation)
            .collect::<Vec<_>>();
        let plan = ReadModelWritePlan::new(mutations);
        plan.validate()?;
        Ok(plan)
    }

    pub async fn commit<S>(self, store: &S) -> Result<ReadModelCommitOutcome, ReadModelError>
    where
        S: ReadModelWritePlanStore + ?Sized,
    {
        store.commit_write_plan(self.into_write_plan()?).await
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
        validate_key(schema, &key)?;
        let expected_version = expected_version.unwrap_or_else(|| self.expected_for(schema, &key));
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
            parent_schema,
            &parent_row,
            relationship,
            child_schema,
            &mut child_row,
        )?;
        let key = key_from_row(child_schema, &child_row)?;
        let expected_version = match mode {
            RowWriteMode::Insert => ExpectedVersion::NotExists,
            RowWriteMode::Upsert => self.expected_for(child_schema, &key),
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
        validate_key(schema, &key)?;
        let expected_version = self.expected_for(schema, &key);
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

    pub(super) fn push(&mut self, mutation: ReadModelMutation) {
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

pub(super) fn validated_schema<M>() -> Result<&'static ReadModelSchema, ReadModelError>
where
    M: RelationalReadModel,
{
    let schema = M::schema();
    schema.validate()?;
    Ok(schema)
}

pub(super) fn populate_delegated_relationship_values(
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
