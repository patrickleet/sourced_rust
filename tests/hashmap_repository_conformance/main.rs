#[path = "../persistent_repository_conformance/mod.rs"]
mod conformance;

use distributed::HashMapRepository;

fn repository() -> HashMapRepository {
    HashMapRepository::new()
}

#[tokio::test]
async fn aggregate_checkout_flow_persists_reloaded_state() {
    conformance::scenario::aggregate_checkout_flow_persists_reloaded_state(repository()).await;
}

#[tokio::test]
async fn get_all_and_commit_all_round_trip() {
    conformance::scenario::get_all_and_commit_all_round_trip(repository()).await;
}

#[tokio::test]
async fn multi_stream_conflict_rolls_back_other_stream_and_snapshot() {
    conformance::scenario::multi_stream_conflict_rolls_back_other_stream_and_snapshot(repository())
        .await;
}

#[tokio::test]
async fn duplicate_stream_identity_is_rejected_before_write() {
    conformance::scenario::duplicate_stream_identity_is_rejected_before_write(repository()).await;
}

#[tokio::test]
async fn metadata_round_trips() {
    conformance::scenario::metadata_round_trips(repository()).await;
}

#[tokio::test]
async fn unsupported_codec_is_rejected_on_write() {
    conformance::scenario::unsupported_codec_is_rejected_on_write(repository()).await;
}

#[tokio::test]
async fn snapshots_use_full_stream_identity() {
    conformance::scenario::snapshots_use_full_stream_identity(repository()).await;
}

#[tokio::test]
async fn standalone_relational_write_plan_persists_row() {
    conformance::read_models::standalone_relational_write_plan_persists_row(repository()).await;
}

#[tokio::test]
async fn aggregate_commit_persists_read_model_plan() {
    conformance::read_models::aggregate_commit_persists_read_model_plan(repository()).await;
}

#[tokio::test]
async fn aggregate_conflict_rolls_back_read_model_plan() {
    conformance::read_models::aggregate_conflict_rolls_back_read_model_plan(repository()).await;
}

#[tokio::test]
async fn high_level_outbox_commit_persists_row_without_stream() {
    let repo = repository();
    conformance::outbox::high_level_outbox_commit_persists_row_without_stream(
        repo.clone(),
        repo.outbox_store(),
    )
    .await;
}

#[tokio::test]
async fn duplicate_outbox_insert_rolls_back_aggregate() {
    conformance::outbox::duplicate_outbox_insert_rolls_back_aggregate(repository()).await;
}

#[tokio::test]
async fn aggregate_conflict_rolls_back_outbox() {
    let repo = repository();
    conformance::outbox::aggregate_conflict_rolls_back_outbox(repo.clone(), repo.outbox_store())
        .await;
}

#[tokio::test]
async fn worker_claim_complete_and_retry_lifecycle() {
    let repo = repository();
    conformance::outbox::worker_claim_complete_and_retry_lifecycle(
        repo.clone(),
        repo.outbox_store(),
    )
    .await;
}

#[tokio::test]
async fn worker_claim_by_ids_claims_only_requested() {
    let repo = repository();
    conformance::outbox::worker_claim_by_ids_claims_only_requested(
        repo.clone(),
        repo.outbox_store(),
    )
    .await;
}

#[tokio::test]
async fn consumer_inbox_records_dedupes_and_fences_with_real_effects() {
    let repo = repository();
    conformance::inbox::inbox_records_dedupes_and_fences_with_real_effects(
        repo.clone(),
        repo.outbox_store(),
    )
    .await;
}

#[tokio::test]
async fn consumer_inbox_rejects_empty_receipt() {
    conformance::inbox::inbox_rejects_empty_receipt(repository()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn racing_commits_one_wins_one_conflicts() {
    conformance::scenario::racing_commits_one_wins_one_conflicts(repository(), 8).await;
}

#[tokio::test]
async fn expired_outbox_lease_is_reclaimed_by_second_worker() {
    let repo = repository();
    conformance::outbox::expired_outbox_lease_is_reclaimed_by_second_worker(
        repo.clone(),
        repo.outbox_store(),
    )
    .await;
}

#[tokio::test]
async fn publish_failure_after_commit_retains_outbox_row_until_delivered() {
    let repo = repository();
    conformance::outbox::publish_failure_after_commit_retains_outbox_row_until_delivered(
        repo.clone(),
        repo.outbox_store(),
    )
    .await;
}
