#![cfg(feature = "sqlite")]

#[path = "../persistent_repository_conformance/mod.rs"]
mod conformance;

use sourced_rust::SqliteRepository;

async fn repository() -> SqliteRepository {
    SqliteRepository::connect_and_migrate("sqlite::memory:")
        .await
        .expect("sqlite conformance repository should migrate")
}

#[tokio::test]
async fn aggregate_checkout_flow_persists_reloaded_state() {
    conformance::scenario::aggregate_checkout_flow_persists_reloaded_state(repository().await)
        .await;
}

#[tokio::test]
async fn get_all_and_commit_all_round_trip() {
    conformance::scenario::get_all_and_commit_all_round_trip(repository().await).await;
}

#[tokio::test]
async fn multi_stream_conflict_rolls_back_other_stream_and_snapshot() {
    conformance::scenario::multi_stream_conflict_rolls_back_other_stream_and_snapshot(
        repository().await,
    )
    .await;
}

#[tokio::test]
async fn duplicate_stream_identity_is_rejected_before_write() {
    conformance::scenario::duplicate_stream_identity_is_rejected_before_write(repository().await)
        .await;
}

#[tokio::test]
async fn metadata_round_trips() {
    conformance::scenario::metadata_round_trips(repository().await).await;
}

#[tokio::test]
async fn unsupported_codec_is_rejected_on_write() {
    conformance::scenario::unsupported_codec_is_rejected_on_write(repository().await).await;
}

#[tokio::test]
async fn snapshots_use_full_stream_identity() {
    conformance::scenario::snapshots_use_full_stream_identity(repository().await).await;
}

#[tokio::test]
async fn high_level_outbox_commit_persists_row_without_stream() {
    let repo = repository().await;
    conformance::outbox::high_level_outbox_commit_persists_row_without_stream(
        repo.clone(),
        repo.outbox_store(),
    )
    .await;
}

#[tokio::test]
async fn duplicate_outbox_insert_rolls_back_aggregate() {
    conformance::outbox::duplicate_outbox_insert_rolls_back_aggregate(repository().await).await;
}

#[tokio::test]
async fn aggregate_conflict_rolls_back_outbox() {
    let repo = repository().await;
    conformance::outbox::aggregate_conflict_rolls_back_outbox(repo.clone(), repo.outbox_store())
        .await;
}

#[tokio::test]
async fn worker_claim_complete_and_retry_lifecycle() {
    let repo = repository().await;
    conformance::outbox::worker_claim_complete_and_retry_lifecycle(
        repo.clone(),
        repo.outbox_store(),
    )
    .await;
}

#[tokio::test]
async fn worker_claim_by_ids_claims_only_requested() {
    let repo = repository().await;
    conformance::outbox::worker_claim_by_ids_claims_only_requested(
        repo.clone(),
        repo.outbox_store(),
    )
    .await;
}
