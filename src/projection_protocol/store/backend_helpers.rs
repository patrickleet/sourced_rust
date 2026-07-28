use super::{
    ProjectionChangeKind, ProjectionFailure, ProjectionFailureBatch,
    ProjectionGraphSnapshotRequest, ProjectionMutationKind, ProjectionProtocolError,
    MAX_PROJECTION_QUERY_BATCH_ROWS,
};
use crate::projection_protocol::MAX_PROJECTION_POSITION;
use crate::table::{column_name_for, RelationshipDef, TableMutation, TableSchema};

pub(crate) fn checked_next(
    value: u64,
    domain: &'static str,
) -> Result<u64, ProjectionProtocolError> {
    if value >= MAX_PROJECTION_POSITION {
        return Err(ProjectionProtocolError::PositionOverflow { domain });
    }
    Ok(value + 1)
}

pub(crate) fn table_model_name(mutation: &TableMutation) -> &str {
    match mutation {
        TableMutation::UpsertRow(mutation) => &mutation.schema.model_name,
        TableMutation::PatchRow(mutation) => &mutation.schema.model_name,
        TableMutation::DeleteRow(mutation) => &mutation.schema.model_name,
    }
}

pub(crate) fn change_kind_for_mutation(kind: ProjectionMutationKind) -> ProjectionChangeKind {
    match kind {
        ProjectionMutationKind::Upsert => ProjectionChangeKind::RecordUpsert,
        ProjectionMutationKind::Delete => ProjectionChangeKind::RecordDelete,
        ProjectionMutationKind::Recreate => ProjectionChangeKind::RecordRecreate,
    }
}

pub(crate) fn failure_matches_batch(
    failure: &ProjectionFailure,
    batch: &ProjectionFailureBatch,
) -> bool {
    failure.input == batch.input.cursor
        && failure.input_fingerprint == batch.input.fingerprint
        && failure.message_id == batch.input.message_id
        && failure.causation_id == batch.input.causation_id
        && failure.gap_free == batch.input.gap_free
        && failure.generation == batch.input.generation
        && failure.failure_code == batch.failure_code
        && failure.failure_bytes == batch.failure_bytes
        && failure.failure_digest == batch.failure_digest
        && failure.change.epoch() == &batch.change_epoch
}

pub(crate) fn validate_projection_graph_snapshot_request(
    request: &ProjectionGraphSnapshotRequest,
) -> Result<(), ProjectionProtocolError> {
    request.root.validate()?;
    if request.max_unique_record_scopes == 0
        || request.max_unique_record_scopes > MAX_PROJECTION_QUERY_BATCH_ROWS
    {
        return Err(ProjectionProtocolError::InvalidBatch(format!(
            "projection graph snapshot record-scope budget is {}; expected 1..={MAX_PROJECTION_QUERY_BATCH_ROWS}",
            request.max_unique_record_scopes
        )));
    }
    let query_scopes = request.includes.len().checked_add(1).ok_or_else(|| {
        ProjectionProtocolError::InvalidBatch(
            "projection graph snapshot query-scope count overflowed".into(),
        )
    })?;
    if query_scopes > MAX_PROJECTION_QUERY_BATCH_ROWS {
        return Err(ProjectionProtocolError::InvalidBatch(format!(
            "projection graph snapshot has {query_scopes} query scopes; maximum is {MAX_PROJECTION_QUERY_BATCH_ROWS}"
        )));
    }
    for (name, include) in &request.includes {
        include.target_schema.validate()?;
        let relationship = request
            .root
            .schema
            .relationships
            .iter()
            .find(|relationship| relationship.field_name == *name)
            .ok_or_else(|| {
                ProjectionProtocolError::InvalidBatch(format!(
                    "projection graph snapshot model `{}` has no relationship `{name}`",
                    request.root.schema.model_name
                ))
            })?;
        if relationship != &include.relationship
            || relationship.target_model != include.target_schema.model_name
            || matches!(
                relationship.kind,
                crate::table::RelationshipKind::ManyToMany
            )
        {
            return Err(ProjectionProtocolError::InvalidBatch(format!(
                "projection graph relationship `{name}` metadata is invalid or divergent"
            )));
        }
    }
    Ok(())
}

pub(crate) fn projection_has_many_columns(
    root_schema: &TableSchema,
    relationship: &RelationshipDef,
    target_schema: &TableSchema,
) -> Result<(String, String), ProjectionProtocolError> {
    let foreign_key = relationship.foreign_key.as_deref().ok_or_else(|| {
        ProjectionProtocolError::InvalidBatch(format!(
            "projection graph relationship `{}` has no foreign key",
            relationship.field_name
        ))
    })?;
    let target_column = column_name_for(target_schema, foreign_key).ok_or_else(|| {
        ProjectionProtocolError::InvalidBatch(format!(
            "projection graph relationship `{}` foreign key `{foreign_key}` is not a target column",
            relationship.field_name
        ))
    })?;
    let target = target_schema
        .columns
        .iter()
        .find(|column| column.column_name == target_column)
        .expect("column_name_for returned a registered target column");
    let root_column = match target.foreign_key.as_ref() {
        Some(reference) if reference.table == root_schema.table_name => {
            column_name_for(root_schema, &reference.column).ok_or_else(|| {
                ProjectionProtocolError::InvalidBatch(format!(
                    "projection graph relationship `{}` targets missing root column `{}`",
                    relationship.field_name, reference.column
                ))
            })?
        }
        Some(reference) => {
            return Err(ProjectionProtocolError::InvalidBatch(format!(
                "projection graph relationship `{}` target column `{target_column}` references table `{}`, not root table `{}`",
                relationship.field_name, reference.table, root_schema.table_name
            )));
        }
        None => {
            let [primary_key] = root_schema.primary_key.columns.as_slice() else {
                return Err(ProjectionProtocolError::InvalidBatch(format!(
                    "projection graph relationship `{}` needs a target-column foreign-key reference because root model `{}` has a composite key",
                    relationship.field_name, root_schema.model_name
                )));
            };
            primary_key.clone()
        }
    };
    Ok((target_column, root_column))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projection_protocol::{
        ProjectionChangeCursor, ProjectionEpoch, ProjectionGeneration, ProjectionInputCursor,
        ProjectionInputFingerprint, ProjectionPartition, ProjectionSource, ProjectorTopologyId,
        TrustedProjectionInput,
    };

    fn topology() -> ProjectorTopologyId {
        ProjectorTopologyId::new(1, "todos", [7; 32]).unwrap()
    }

    fn partition() -> ProjectionPartition {
        ProjectionPartition::new(b"tenant:a".to_vec()).unwrap()
    }

    fn failure_batch() -> ProjectionFailureBatch {
        let input = TrustedProjectionInput::mint(
            ProjectionInputCursor::new(
                topology(),
                partition(),
                ProjectionSource::new("todo_stream", b"todo-1".to_vec()).unwrap(),
                ProjectionEpoch::new("source-v1").unwrap(),
                1,
            )
            .unwrap(),
            ProjectionInputFingerprint::from_canonical_bytes(b"input-1"),
            "message-1",
            "cause-1",
            ProjectionGeneration::initial(),
            true,
        )
        .unwrap();
        ProjectionFailureBatch::new(
            input,
            ProjectionEpoch::new("changes-v1").unwrap(),
            "failure-1",
            "decode_error",
            b"bad payload".to_vec(),
        )
        .unwrap()
    }

    #[test]
    fn failure_matches_batch_rejects_mismatched_gap_free_identity() {
        let batch = failure_batch();
        let mut failure = ProjectionFailure {
            failure_id: batch.failure_id.clone(),
            input: batch.input.cursor.clone(),
            input_fingerprint: batch.input.fingerprint.clone(),
            message_id: batch.input.message_id.clone(),
            causation_id: batch.input.causation_id.clone(),
            generation: batch.input.generation,
            gap_free: !batch.input.gap_free,
            failure_code: batch.failure_code.clone(),
            failure_bytes: batch.failure_bytes.clone(),
            failure_digest: batch.failure_digest,
            change: ProjectionChangeCursor::new(
                topology(),
                partition(),
                batch.change_epoch.clone(),
                1,
            )
            .unwrap(),
        };

        assert!(!failure_matches_batch(&failure, &batch));

        failure.gap_free = batch.input.gap_free;
        assert!(failure_matches_batch(&failure, &batch));
    }
}
