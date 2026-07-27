use super::*;

pub(super) fn read_projection_query_snapshot_from_state(
    rows: &HashMap<String, StoredRow>,
    protocol: &InMemoryProjectionProtocolState,
    request: &ProjectionQuerySnapshotRequest,
) -> Result<ProjectionQuerySnapshot, ProjectionProtocolError> {
    request.validate()?;
    let registered_key = RegisteredModelKey {
        topology: request.scope.topology().clone(),
        model: request.scope.model().to_string(),
    };
    if protocol
        .registered_models
        .get(&registered_key)
        .map(String::as_str)
        != Some(request.schema.table_name.as_str())
        || protocol
            .authoritative_table_owners
            .get(&request.schema.table_name)
            != Some(&registered_key)
    {
        return Err(ProjectionProtocolError::InvalidBatch(format!(
            "projection query model `{}` is not the registered owner of table `{}`",
            request.scope.model(),
            request.schema.table_name
        )));
    }

    let storage_key = relational_storage_key(&request.schema.table_name, &request.key);
    let row = rows.get(&storage_key).map(|stored| stored.values.clone());
    let record = protocol.records.get(&request.scope).cloned();
    match (row.is_some(), record.as_ref()) {
        (true, None)
        | (
            true,
            Some(ProjectionRecordMetadata {
                tombstone: true, ..
            }),
        ) => {
            return Err(ProjectionProtocolError::RecordAlreadyExists {
                model: request.scope.model().to_string(),
            });
        }
        (
            false,
            Some(ProjectionRecordMetadata {
                tombstone: false, ..
            }),
        ) => {
            return Err(ProjectionProtocolError::RecordMissing {
                model: request.scope.model().to_string(),
            });
        }
        _ => {}
    }

    let partition_key = PartitionKey::new(
        request.scope.topology(),
        request.scope.projection_partition(),
    );
    let partition = protocol.partitions.get(&partition_key);
    if record.is_some() && partition.is_none() {
        return Err(ProjectionProtocolError::InvalidBatch(
            "projection query record exists without partition state".into(),
        ));
    }

    let (change_head, compacted_through) = match partition {
        Some(partition) => {
            if let Some(record) = &record {
                if record.change.epoch() != &partition.change_epoch
                    || record.change.position() > partition.change_head
                {
                    return Err(ProjectionProtocolError::InvalidBatch(
                        "projection query record change lies outside its partition head".into(),
                    ));
                }
            }
            let head = if partition.change_head == 0 {
                None
            } else {
                Some(ProjectionChangeCursor::new(
                    request.scope.topology().clone(),
                    request.scope.projection_partition().clone(),
                    partition.change_epoch.clone(),
                    partition.change_head,
                )?)
            };
            (head, partition.compacted_through)
        }
        None => (None, 0),
    };

    let mut checkpoints = Vec::with_capacity(request.checkpoint_probes.len());
    for probe in &request.checkpoint_probes {
        let stored = protocol.inputs.get(&InputKey {
            partition: partition_key.clone(),
            source: probe.source.clone(),
            generation: probe.generation,
        });
        let checkpoint = match stored {
            Some(stored) => {
                let Some(partition) = partition else {
                    return Err(ProjectionProtocolError::InvalidBatch(
                        "projection checkpoint exists without partition state".into(),
                    ));
                };
                if stored.cursor.epoch() != &probe.epoch {
                    return Err(ProjectionProtocolError::IncomparableInput);
                }
                if stored.checkpoint.change().epoch() != &partition.change_epoch
                    || stored.checkpoint.change().position() > partition.change_head
                {
                    return Err(ProjectionProtocolError::InvalidBatch(
                        "projection query checkpoint lies outside its partition head".into(),
                    ));
                }
                Some(stored.checkpoint.clone())
            }
            None => None,
        };
        checkpoints.push(crate::projection_protocol::ProjectionCheckpointSnapshot {
            probe: probe.clone(),
            checkpoint,
        });
    }

    Ok(ProjectionQuerySnapshot {
        row,
        record,
        checkpoints,
        change_head,
        compacted_through,
    })
}

pub(super) fn validate_observation_from_state(
    protocol: &InMemoryProjectionProtocolState,
    request: &crate::projection_protocol::ProjectionObligationEvidenceRequest,
    observation: &ProjectionObservation,
) -> Result<(), ProjectionProtocolError> {
    if observation.causation_id != request.causation_id
        || observation.kind != request.kind
        || observation.scope != request.scope
        || observation
            .revision
            .as_ref()
            .is_some_and(|revision| revision.scope() != &request.scope)
        || (request.kind == ProjectionObservationKind::Record) != observation.revision.is_some()
    {
        return Err(ProjectionProtocolError::InvalidBatch(
            "stored projection observation does not match its exact evidence key".into(),
        ));
    }
    let partition = protocol
        .partitions
        .get(&PartitionKey::new(
            request.scope.topology(),
            request.scope.projection_partition(),
        ))
        .ok_or_else(|| {
            ProjectionProtocolError::InvalidBatch(
                "stored projection observation has no partition state".into(),
            )
        })?;
    if observation.change.topology() != request.scope.topology()
        || observation.change.projection_partition() != request.scope.projection_partition()
        || observation.change.epoch() != &partition.change_epoch
        || observation.change.position() > partition.change_head
    {
        return Err(ProjectionProtocolError::InvalidBatch(
            "stored projection observation change lies outside its partition".into(),
        ));
    }
    Ok(())
}

pub(super) fn validate_failure_from_state(
    protocol: &InMemoryProjectionProtocolState,
    request: &crate::projection_protocol::ProjectionObligationEvidenceRequest,
    failure: &ProjectionFailure,
) -> Result<(), ProjectionProtocolError> {
    if failure.causation_id != request.causation_id
        || failure.input.topology() != request.scope.topology()
        || failure.input.projection_partition() != request.scope.projection_partition()
        || failure.change.topology() != request.scope.topology()
        || failure.change.projection_partition() != request.scope.projection_partition()
    {
        return Err(ProjectionProtocolError::InvalidBatch(
            "stored projection failure does not match its evidence scope".into(),
        ));
    }
    let partition = protocol
        .partitions
        .get(&PartitionKey::new(
            request.scope.topology(),
            request.scope.projection_partition(),
        ))
        .ok_or_else(|| {
            ProjectionProtocolError::InvalidBatch(
                "stored projection failure has no partition state".into(),
            )
        })?;
    if failure.change.epoch() != &partition.change_epoch
        || failure.change.position() > partition.change_head
    {
        return Err(ProjectionProtocolError::InvalidBatch(
            "stored projection failure change lies outside its partition".into(),
        ));
    }
    Ok(())
}

pub(super) fn read_projection_obligation_evidence_from_state(
    protocol: &InMemoryProjectionProtocolState,
    request: &crate::projection_protocol::ProjectionObligationEvidenceRequest,
) -> Result<ProjectionObligationEvidence, ProjectionProtocolError> {
    request.validate()?;
    let failure = protocol
        .failures
        .values()
        .filter(|failure| {
            failure.causation_id == request.causation_id
                && failure.input.topology() == request.scope.topology()
                && failure.input.projection_partition() == request.scope.projection_partition()
        })
        .min_by_key(|failure| failure.change.position())
        .cloned();
    if let Some(failure) = failure {
        validate_failure_from_state(protocol, request, &failure)?;
        return Ok(ProjectionObligationEvidence::TerminalFailure(failure));
    }

    let observation = protocol
        .observations
        .get(&ObservationKey {
            causation_id: request.causation_id.clone(),
            scope: request.scope.clone(),
            kind: request.kind,
        })
        .cloned();
    match observation {
        Some(observation) => {
            validate_observation_from_state(protocol, request, &observation)?;
            Ok(ProjectionObligationEvidence::Observed(observation))
        }
        None => Ok(ProjectionObligationEvidence::Pending),
    }
}

#[allow(dead_code)]
pub(super) fn read_projection_live_record_from_state(
    protocol: &InMemoryProjectionProtocolState,
    request: &crate::projection_protocol::ProjectionLiveRecordRequest,
) -> Result<Option<ProjectionRecordMetadata>, ProjectionProtocolError> {
    request.validate()?;
    let registered_key = RegisteredModelKey {
        topology: request.topology.clone(),
        model: request.model().to_string(),
    };
    if protocol
        .registered_models
        .get(&registered_key)
        .map(String::as_str)
        != Some(request.schema.table_name.as_str())
        || protocol
            .authoritative_table_owners
            .get(&request.schema.table_name)
            != Some(&registered_key)
    {
        return Err(ProjectionProtocolError::InvalidBatch(format!(
            "projection live-record model `{}` is not the registered owner of table `{}`",
            request.model(),
            request.schema.table_name
        )));
    }

    let mut live = None;
    for (scope, metadata) in &protocol.records {
        if metadata.tombstone
            || scope.topology() != &request.topology
            || scope.model() != request.model()
            || scope.key_digest() != request.canonical_key_hash
        {
            continue;
        }
        if scope.canonical_key_bytes() != request.canonical_key_bytes {
            return Err(ProjectionProtocolError::InvalidBatch(format!(
                "projection live-record canonical key mismatch for model `{}`",
                request.model()
            )));
        }
        if metadata.revision.scope() != scope {
            return Err(ProjectionProtocolError::InvalidBatch(
                "projection live-record metadata is stored under a different scope".into(),
            ));
        }
        if live.replace(metadata.clone()).is_some() {
            return Err(ProjectionProtocolError::InvalidBatch(format!(
                "projection live-record identity for model `{}` is ambiguous across partitions",
                request.model()
            )));
        }
    }

    if let Some(metadata) = &live {
        let scope = metadata.revision.scope();
        let partition = protocol
            .partitions
            .get(&PartitionKey::new(
                scope.topology(),
                scope.projection_partition(),
            ))
            .ok_or_else(|| {
                ProjectionProtocolError::InvalidBatch(
                    "projection live record has no partition state".into(),
                )
            })?;
        if metadata.change.topology() != scope.topology()
            || metadata.change.projection_partition() != scope.projection_partition()
            || metadata.change.epoch() != &partition.change_epoch
            || metadata.change.position() > partition.change_head
        {
            return Err(ProjectionProtocolError::InvalidBatch(
                "projection live-record change lies outside its partition".into(),
            ));
        }
    }
    Ok(live)
}
