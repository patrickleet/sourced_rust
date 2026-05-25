use std::collections::{HashMap, HashSet};

use crate::entity::{
    EventRecord, EventRecordError, BITCODE_PAYLOAD_CODEC, BITCODE_PAYLOAD_CODEC_VERSION,
};
use crate::outbox::OutboxMessage;
#[cfg(feature = "sqlite")]
use crate::read_model::ReadModelError;
use crate::repository::{AsyncStreamWrite, PreparedEventAppend, RepositoryError, StreamIdentity};
use crate::snapshot::SnapshotRecord;

pub(crate) fn reject_duplicate_streams(
    streams: &[AsyncStreamWrite<'_>],
) -> Result<(), RepositoryError> {
    let mut seen = HashSet::with_capacity(streams.len());
    for stream in streams {
        let key = stream.identity.storage_key();
        if !seen.insert(key) {
            return Err(RepositoryError::DuplicateStreamInBatch {
                id: stream.identity.to_string(),
            });
        }
    }
    Ok(())
}

pub(crate) fn reject_duplicate_outbox_messages(
    messages: &[OutboxMessage],
) -> Result<(), RepositoryError> {
    let mut seen = HashSet::with_capacity(messages.len());
    for message in messages {
        validate_outbox_table_write(message)?;
        let id = message.id();
        if id.trim().is_empty() {
            return Err(RepositoryError::Model(
                "outbox message id must not be empty".into(),
            ));
        }
        if message.event_type.trim().is_empty() {
            return Err(RepositoryError::Model(format!(
                "outbox message `{id}` event type must not be empty"
            )));
        }
        if !seen.insert(id.to_string()) {
            return Err(RepositoryError::DuplicateOutboxMessageInBatch { id: id.into() });
        }
    }
    Ok(())
}

fn validate_outbox_table_write(message: &OutboxMessage) -> Result<(), RepositoryError> {
    crate::outbox::outbox_message_insert_plan(message)
        .and_then(|plan| plan.validate().map(|()| plan))
        .map(|_| ())
        .map_err(|err| RepositoryError::Model(err.to_string()))
}

pub(crate) fn validate_entity_id_matches_identity(
    streams: &[AsyncStreamWrite<'_>],
) -> Result<(), RepositoryError> {
    for stream in streams {
        if stream.entity.id() != stream.identity.aggregate_id() {
            return Err(RepositoryError::Model(format!(
                "stream identity `{}` does not match entity id `{}`",
                stream.identity,
                stream.entity.id()
            )));
        }
    }
    Ok(())
}

pub(crate) fn validate_prepared_appends(
    appends: &[PreparedEventAppend],
) -> Result<(), RepositoryError> {
    for append in appends {
        for (offset, event) in append.events.iter().enumerate() {
            validate_supported_event_codec(event)?;
            let expected_sequence = append.expected_version + offset as u64 + 1;
            if event.sequence != expected_sequence {
                return Err(RepositoryError::Model(format!(
                    "event `{}` for stream `{}` has sequence {}, expected {}",
                    event.event_name, append.identity, event.sequence, expected_sequence
                )));
            }
        }
    }
    Ok(())
}

pub(crate) fn validate_supported_event_codec(event: &EventRecord) -> Result<(), RepositoryError> {
    if event.payload_codec != BITCODE_PAYLOAD_CODEC
        || event.payload_codec_version != BITCODE_PAYLOAD_CODEC_VERSION
    {
        return Err(EventRecordError::unsupported_codec(
            &event.payload_codec,
            event.payload_codec_version,
        )
        .into());
    }
    Ok(())
}

pub(crate) fn serialize_event_metadata(
    metadata: &HashMap<String, String>,
) -> Result<String, RepositoryError> {
    serde_json::to_string(metadata)
        .map_err(|err| RepositoryError::Model(format!("serialize event metadata: {err}")))
}

pub(crate) fn deserialize_event_metadata(
    metadata_json: &str,
) -> Result<HashMap<String, String>, RepositoryError> {
    serde_json::from_str(metadata_json)
        .map_err(|err| RepositoryError::Model(format!("deserialize event metadata: {err}")))
}

pub(crate) fn validate_snapshot_identity(
    identity: &StreamIdentity,
    record: &SnapshotRecord,
) -> Result<(), RepositoryError> {
    if record.aggregate_id != identity.aggregate_id() {
        return Err(RepositoryError::Model(format!(
            "snapshot aggregate id `{}` does not match stream identity `{}`",
            record.aggregate_id, identity
        )));
    }
    Ok(())
}

pub(crate) fn repository_i64_from_u64(
    backend: &str,
    value: u64,
    field: &str,
    storage: &str,
) -> Result<i64, RepositoryError> {
    i64::try_from(value).map_err(|_| {
        RepositoryError::Model(format!("{backend} {field} value {value} exceeds {storage}"))
    })
}

#[cfg(feature = "postgres")]
pub(crate) fn repository_i32_from_u64(
    backend: &str,
    value: u64,
    field: &str,
    storage: &str,
) -> Result<i32, RepositoryError> {
    i32::try_from(value).map_err(|_| {
        RepositoryError::Model(format!("{backend} {field} value {value} exceeds {storage}"))
    })
}

pub(crate) fn repository_u64_from_i64(
    backend: &str,
    value: i64,
    field: &str,
) -> Result<u64, RepositoryError> {
    u64::try_from(value)
        .map_err(|_| RepositoryError::Model(format!("{backend} {field} value {value} is negative")))
}

#[cfg(feature = "postgres")]
pub(crate) fn repository_u64_from_i32(
    backend: &str,
    value: i32,
    field: &str,
) -> Result<u64, RepositoryError> {
    u64::try_from(value)
        .map_err(|_| RepositoryError::Model(format!("{backend} {field} value {value} is negative")))
}

#[cfg(feature = "sqlite")]
pub(crate) fn repository_u16_from_i64(
    backend: &str,
    value: i64,
    field: &str,
) -> Result<u16, RepositoryError> {
    u16::try_from(value)
        .map_err(|_| RepositoryError::Model(format!("{backend} {field} value {value} is invalid")))
}

#[cfg(feature = "postgres")]
pub(crate) fn repository_u16_from_i32(
    backend: &str,
    value: i32,
    field: &str,
) -> Result<u16, RepositoryError> {
    u16::try_from(value)
        .map_err(|_| RepositoryError::Model(format!("{backend} {field} value {value} is invalid")))
}

#[cfg(feature = "sqlite")]
pub(crate) fn read_model_i64_from_u64(
    backend: &str,
    value: u64,
    field: &str,
    storage: &str,
) -> Result<i64, ReadModelError> {
    i64::try_from(value).map_err(|_| {
        ReadModelError::Storage(format!("{backend} {field} value {value} exceeds {storage}"))
    })
}

#[cfg(feature = "sqlite")]
pub(crate) fn read_model_u64_from_i64(
    backend: &str,
    value: i64,
    field: &str,
) -> Result<u64, ReadModelError> {
    u64::try_from(value).map_err(|_| {
        ReadModelError::Storage(format!("{backend} {field} value {value} is negative"))
    })
}

#[cfg(feature = "sqlite")]
pub(crate) fn is_sqlite_unique_constraint(err: &sqlx::Error) -> bool {
    match err {
        sqlx::Error::Database(db_err) => {
            let message = db_err.message();
            let code = db_err.code().map(|code| code.into_owned());
            message.contains("UNIQUE constraint failed")
                || message.contains("PRIMARY KEY")
                || matches!(code.as_deref(), Some("1555" | "2067"))
        }
        _ => false,
    }
}

#[cfg(feature = "postgres")]
pub(crate) fn is_postgres_unique_violation(err: &sqlx::Error) -> bool {
    match err {
        sqlx::Error::Database(db_err) => db_err.code().as_deref() == Some("23505"),
        _ => false,
    }
}

pub(crate) fn repository_storage_error(
    backend: &str,
    operation: &str,
    err: sqlx::Error,
) -> RepositoryError {
    RepositoryError::Model(format!("{backend} {operation} failed: {err}"))
}

#[cfg(feature = "sqlite")]
pub(crate) fn read_model_storage_error(
    backend: &str,
    operation: &str,
    err: sqlx::Error,
) -> ReadModelError {
    ReadModelError::Storage(format!("{backend} {operation} failed: {err}"))
}
