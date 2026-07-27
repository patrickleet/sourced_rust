#![expect(
    clippy::manual_async_fn,
    reason = "trait impls preserve the ProjectionProtocolStore Send bounds"
)]

use std::collections::{BTreeMap, HashMap, HashSet};
use std::future::Future;

use super::InMemoryRepository;
use crate::projection_protocol::{
    change_kind_for_mutation, checked_next, failure_matches_batch, table_model_name,
    ProjectionChange, ProjectionChangeCursor, ProjectionChangeKind, ProjectionChangeRead,
    ProjectionChangeRetention, ProjectionCheckpoint, ProjectionCommitBatch,
    ProjectionCommitOutcome, ProjectionCommitResult, ProjectionEpoch, ProjectionFailure,
    ProjectionFailureBatch, ProjectionFailureLocation, ProjectionGeneration, ProjectionInputCursor,
    ProjectionInputDisposition, ProjectionInputFingerprint, ProjectionLiveRecordBatch,
    ProjectionLiveRecordBatchRequest, ProjectionMutationKind, ProjectionObligationEvidence,
    ProjectionObligationEvidenceBatch, ProjectionObligationEvidenceBatchRequest,
    ProjectionObservation, ProjectionObservationKind, ProjectionObservationTarget,
    ProjectionPartition, ProjectionPartitionRuntimeState, ProjectionPendingRetry,
    ProjectionProtocolError, ProjectionProtocolStore, ProjectionQuerySnapshot,
    ProjectionQuerySnapshotBatch, ProjectionQuerySnapshotBatchRequest,
    ProjectionQuerySnapshotRequest, ProjectionRecordExpectation, ProjectionRecordMetadata,
    ProjectionRecordScope, ProjectionSource, ProjectorTopologyId, RecordRevision,
    RevisionComparison, SameTransactionProjectionBatch, SameTransactionProjectionEvidence,
    TrustedProjectionInput,
};
use crate::read_model::in_memory::{
    apply_read_model_write_plan, relational_storage_key, StoredRow,
};
use crate::repository::RepositoryError;
use crate::table::{TableMutation, TableStoreError, TableWritePlan};

mod direct_projection;
mod read_helpers;
mod state;
mod state_impl;
mod store_impl;
mod util;

pub(super) use direct_projection::stage_same_transaction_projection;
pub(super) use state::{reject_causal_owned_plans, InMemoryProjectionProtocolState};

use read_helpers::{
    read_projection_live_record_from_state, read_projection_obligation_evidence_from_state,
    read_projection_query_snapshot_from_state,
};
use state::*;
use util::storage_key_belongs_to_table;

#[cfg(test)]
mod tests;
