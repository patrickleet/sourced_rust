use super::*;

pub(super) fn table_model_name(mutation: &TableMutation) -> &str {
    match mutation {
        TableMutation::UpsertRow(mutation) => &mutation.schema.model_name,
        TableMutation::PatchRow(mutation) => &mutation.schema.model_name,
        TableMutation::DeleteRow(mutation) => &mutation.schema.model_name,
    }
}

pub(super) fn storage_key_belongs_to_table(storage_key: &str, table: &str) -> bool {
    let Some(mut fingerprint) = storage_key
        .strip_prefix(table)
        .and_then(|suffix| suffix.strip_prefix(':'))
    else {
        return false;
    };
    if fingerprint.is_empty() {
        return false;
    }
    while !fingerprint.is_empty() {
        let Some(length_end) = fingerprint.find(':') else {
            return false;
        };
        let Ok(part_length) = fingerprint[..length_end].parse::<usize>() else {
            return false;
        };
        let part_start = length_end + 1;
        let Some(part_end) = part_start.checked_add(part_length) else {
            return false;
        };
        if fingerprint.as_bytes().get(part_end) != Some(&b';')
            || !fingerprint.is_char_boundary(part_end + 1)
        {
            return false;
        }
        fingerprint = &fingerprint[part_end + 1..];
    }
    true
}

pub(super) fn checked_next(
    value: u64,
    domain: &'static str,
) -> Result<u64, ProjectionProtocolError> {
    if value >= MAX_PROJECTION_POSITION {
        return Err(ProjectionProtocolError::PositionOverflow { domain });
    }
    Ok(value + 1)
}

pub(super) fn failure_matches_batch(
    failure: &ProjectionFailure,
    batch: &ProjectionFailureBatch,
) -> bool {
    failure.input == batch.input.cursor
        && failure.input_fingerprint == batch.input.fingerprint
        && failure.message_id == batch.input.message_id
        && failure.causation_id == batch.input.causation_id
        && failure.generation == batch.input.generation
        && failure.failure_code == batch.failure_code
        && failure.failure_bytes == batch.failure_bytes
        && failure.failure_digest == batch.failure_digest
        && failure.change.epoch() == &batch.change_epoch
}
