//! Consumer inbox — the optional consumer-side effect fence.
//!
//! The inbox is the consumer-side complement to the producer outbox: an optional
//! durable receipt that lets a consumer get **effectively-once local database
//! effects** on top of at-least-once transport delivery. It is *not* a read-model
//! feature (it replaces the removed `read_model_processed_messages`, which wrongly
//! coupled delivery dedupe to the projection contract — see
//! `specs/consumer-inbox-design.md`).
//!
//! An [`InboxReceipt`] is a participant in the transactional commit batch
//! (alongside aggregate events, outbox rows, read-model write plans, and
//! snapshots). The relational stores write it to an operational `consumer_inbox`
//! table in the **same transaction** as everything else in the batch — that
//! atomicity is what makes the fence real: handler effects and the receipt land
//! together or not at all.
//!
//! Default consumers stay idempotent (a replayed projection re-converges); the
//! inbox is the opt-in pattern for when exactly-once local effects are worth the
//! cost. It only fences *local transactional* effects — external side effects
//! (HTTP calls, etc.) still require handler idempotency.

use std::time::SystemTime;

/// A single consumer/message processing receipt.
///
/// Identified by `(consumer, message_id)`; committed atomically with the
/// consumer's other writes so a redelivery of the same message is a no-op.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InboxReceipt {
    /// The logical consumer (e.g. a projection or service name) — the dedupe scope.
    pub consumer: String,
    /// The transport message's stable id.
    pub message_id: String,
    /// When the receipt was created.
    pub processed_at: SystemTime,
}

impl InboxReceipt {
    /// Create a receipt for `(consumer, message_id)`, stamped now.
    pub fn new(consumer: impl Into<String>, message_id: impl Into<String>) -> Self {
        Self {
            consumer: consumer.into(),
            message_id: message_id.into(),
            processed_at: SystemTime::now(),
        }
    }

    /// The `(consumer, message_id)` dedupe key.
    pub fn key(&self) -> (&str, &str) {
        (&self.consumer, &self.message_id)
    }
}

/// Outcome of committing an [`InboxReceipt`].
///
/// A `Duplicate` is **not** an error: the message was already processed, so the
/// consumer treats it as success (acks) without re-running effects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InboxOutcome {
    /// The receipt was newly recorded (first time this message was processed).
    Processed,
    /// A receipt for `(consumer, message_id)` already existed; effects were skipped.
    Duplicate,
}

impl InboxOutcome {
    /// Whether this is the first processing (vs a deduplicated replay).
    pub fn is_processed(self) -> bool {
        matches!(self, InboxOutcome::Processed)
    }

    /// Whether the message was already processed (a deduplicated replay).
    pub fn is_duplicate(self) -> bool {
        matches!(self, InboxOutcome::Duplicate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn receipt_key_is_consumer_and_message_id() {
        let r = InboxReceipt::new("projections", "evt-1");
        assert_eq!(r.key(), ("projections", "evt-1"));
        assert_eq!(r.consumer, "projections");
        assert_eq!(r.message_id, "evt-1");
    }

    #[test]
    fn outcome_predicates() {
        assert!(InboxOutcome::Processed.is_processed());
        assert!(!InboxOutcome::Processed.is_duplicate());
        assert!(InboxOutcome::Duplicate.is_duplicate());
        assert!(!InboxOutcome::Duplicate.is_processed());
    }
}
