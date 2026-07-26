use super::{ProjectionChangeKind, ProjectionMutationKind, ProjectionProtocolError};
use crate::projection_protocol::MAX_PROJECTION_POSITION;
use crate::table::TableMutation;

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
