//! Write plans: the deterministic mutation batches table stores apply.

use super::mutation::{validate_delete_mutation, validate_patch_mutation, validate_row_mutation};
use super::{TableMutation, TableStoreError};

/// Adapter capabilities used to validate a write plan before any storage write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableAdapterCapabilities {
    pub relational_rows: bool,
    pub sparse_patches: bool,
    pub deletes: bool,
}

impl Default for TableAdapterCapabilities {
    fn default() -> Self {
        Self {
            relational_rows: true,
            sparse_patches: true,
            deletes: true,
        }
    }
}

/// Result of applying a standalone table write plan.
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
pub struct TableCommitOutcome;

impl TableCommitOutcome {
    /// The write plan was applied. Currently the only outcome (see the type docs).
    pub fn applied() -> Self {
        Self
    }

    /// Always `true` today — see the type docs for why there is no skipped variant.
    pub fn was_applied(&self) -> bool {
        true
    }
}

/// Deterministic unit-of-work output for relational table-store adapters.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TableWritePlan {
    pub mutations: Vec<TableMutation>,
}

impl TableWritePlan {
    pub fn new(mutations: Vec<TableMutation>) -> Self {
        Self { mutations }
    }

    pub fn is_empty(&self) -> bool {
        self.mutations.is_empty()
    }

    pub fn validate(&self) -> Result<(), TableStoreError> {
        self.validate_for(&TableAdapterCapabilities::default())
    }

    pub fn validate_for(
        &self,
        capabilities: &TableAdapterCapabilities,
    ) -> Result<(), TableStoreError> {
        for mutation in &self.mutations {
            match mutation {
                TableMutation::UpsertRow(mutation) => {
                    if !capabilities.relational_rows {
                        return Err(TableStoreError::Metadata(
                            "read-model adapter does not support relational row writes".into(),
                        ));
                    }
                    validate_row_mutation(mutation)?;
                }
                TableMutation::PatchRow(mutation) => {
                    if !capabilities.relational_rows || !capabilities.sparse_patches {
                        return Err(TableStoreError::Metadata(
                            "read-model adapter does not support sparse row patches".into(),
                        ));
                    }
                    validate_patch_mutation(mutation)?;
                }
                TableMutation::DeleteRow(mutation) => {
                    if !capabilities.relational_rows || !capabilities.deletes {
                        return Err(TableStoreError::Metadata(
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
