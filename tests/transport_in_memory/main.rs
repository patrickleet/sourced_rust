//! In-memory reference run of the async-transport conformance harness.
//!
//! Proves the shared transport contract (source runner + outbox dispatcher)
//! against adapter-neutral fakes, with no external broker. Concrete adapters
//! reuse the harness in [`transport_conformance`] for their own targets.

#[path = "../transport_conformance/mod.rs"]
mod conformance;

// --- source runner contract -------------------------------------------------

#[tokio::test]
async fn source_dispatches_before_ack() {
    conformance::source_dispatches_before_ack().await;
}

#[tokio::test]
async fn source_retryable_failure_nacks_without_ack() {
    conformance::source_retryable_failure_nacks_without_ack().await;
}

#[tokio::test]
async fn source_permanent_failure_dead_letters_by_default() {
    conformance::source_permanent_failure_dead_letters_by_default().await;
}

#[tokio::test]
async fn source_permanent_failure_stops_under_stop_policy() {
    conformance::source_permanent_failure_stops_under_stop_policy().await;
}

#[tokio::test]
async fn source_unhandled_message_is_acked_and_ignored() {
    conformance::source_unhandled_message_is_acked_and_ignored().await;
}

#[tokio::test]
async fn source_inbox_mode_rejects_missing_stable_id() {
    conformance::source_inbox_mode_rejects_missing_stable_id().await;
}

#[tokio::test]
async fn source_inbox_mode_dispatches_with_stable_id() {
    conformance::source_inbox_mode_dispatches_with_stable_id().await;
}

#[tokio::test]
async fn source_propagates_recv_errors() {
    conformance::source_propagates_recv_errors().await;
}

#[tokio::test]
async fn source_propagates_settle_errors() {
    conformance::source_propagates_settle_errors().await;
}

// --- publisher / outbox dispatcher contract ---------------------------------

#[tokio::test]
async fn dispatcher_completes_only_after_publish_success() {
    conformance::dispatcher_completes_only_after_publish_success().await;
}

#[tokio::test]
async fn dispatcher_unknown_outcome_stays_retryable() {
    conformance::dispatcher_unknown_outcome_stays_retryable().await;
}

#[tokio::test]
async fn dispatcher_claims_explicit_ids_before_publish() {
    conformance::dispatcher_claims_explicit_ids_before_publish().await;
}
