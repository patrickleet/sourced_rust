//! Backend-agnostic commit-batch validation.
//!
//! These functions define what a *valid* [`CommitBatch`](super::CommitBatch) is:
//! they validate the repository-level types ([`StreamWrite`], [`PreparedEventAppend`],
//! [`OutboxMessage`], and snapshot identities). They have no storage-backend
//! dependency, so every backend (in-memory, sqlite, postgres) imports this single
//! copy. Keeping one definition here is what prevents the three backends from
//! drifting on what a valid commit batch is.

use std::collections::HashSet;

use crate::entity::{
    EventRecord, EventRecordError, BITCODE_PAYLOAD_CODEC, BITCODE_PAYLOAD_CODEC_VERSION,
};
use crate::outbox::{validate_outbox_message_table_write, OutboxMessage};
use crate::snapshot::SnapshotRecord;

use super::{PreparedEventAppend, RepositoryError, StreamIdentity, StreamWrite};

pub(crate) fn reject_duplicate_streams(streams: &[StreamWrite<'_>]) -> Result<(), RepositoryError> {
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
        validate_outbox_message_table_write(message)
            .map_err(|err| RepositoryError::Model(err.to_string()))?;
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

pub(crate) fn validate_entity_id_matches_identity(
    streams: &[StreamWrite<'_>],
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

pub(crate) fn validate_snapshot_identity(
    identity: &StreamIdentity,
    record: &SnapshotRecord,
) -> Result<(), RepositoryError> {
    record.validate_for_identity(identity)
}
