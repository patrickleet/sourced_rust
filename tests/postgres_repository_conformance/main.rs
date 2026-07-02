#![cfg(feature = "postgres")]

#[path = "../persistent_repository_conformance/mod.rs"]
mod conformance;
#[path = "../support/postgres.rs"]
mod postgres;

use distributed::PostgresRepository;

async fn repository() -> Option<PostgresRepository> {
    let schema = postgres::PostgresTestSchema::create_from_env(
        "pg_conformance",
        "skipping Postgres conformance test",
    )
    .await?;
    Some(schema.repository().await)
}

#[tokio::test]
async fn aggregate_checkout_flow_persists_reloaded_state() {
    let Some(repo) = repository().await else {
        return;
    };
    conformance::scenario::aggregate_checkout_flow_persists_reloaded_state(repo).await;
}

#[tokio::test]
async fn get_all_and_commit_all_round_trip() {
    let Some(repo) = repository().await else {
        return;
    };
    conformance::scenario::get_all_and_commit_all_round_trip(repo).await;
}

#[tokio::test]
async fn multi_stream_conflict_rolls_back_other_stream_and_snapshot() {
    let Some(repo) = repository().await else {
        return;
    };
    conformance::scenario::multi_stream_conflict_rolls_back_other_stream_and_snapshot(repo).await;
}

#[tokio::test]
async fn duplicate_stream_identity_is_rejected_before_write() {
    let Some(repo) = repository().await else {
        return;
    };
    conformance::scenario::duplicate_stream_identity_is_rejected_before_write(repo).await;
}

#[tokio::test]
async fn metadata_round_trips() {
    let Some(repo) = repository().await else {
        return;
    };
    conformance::scenario::metadata_round_trips(repo).await;
}

#[tokio::test]
async fn unsupported_codec_is_rejected_on_write() {
    let Some(repo) = repository().await else {
        return;
    };
    conformance::scenario::unsupported_codec_is_rejected_on_write(repo).await;
}

#[tokio::test]
async fn snapshots_use_full_stream_identity() {
    let Some(repo) = repository().await else {
        return;
    };
    conformance::scenario::snapshots_use_full_stream_identity(repo).await;
}

#[tokio::test]
async fn high_level_outbox_commit_persists_row_without_stream() {
    let Some(repo) = repository().await else {
        return;
    };
    conformance::outbox::high_level_outbox_commit_persists_row_without_stream(
        repo.clone(),
        repo.outbox_store(),
    )
    .await;
}

#[tokio::test]
async fn duplicate_outbox_insert_rolls_back_aggregate() {
    let Some(repo) = repository().await else {
        return;
    };
    conformance::outbox::duplicate_outbox_insert_rolls_back_aggregate(repo).await;
}

#[tokio::test]
async fn aggregate_conflict_rolls_back_outbox() {
    let Some(repo) = repository().await else {
        return;
    };
    conformance::outbox::aggregate_conflict_rolls_back_outbox(repo.clone(), repo.outbox_store())
        .await;
}

#[tokio::test]
async fn worker_claim_complete_and_retry_lifecycle() {
    let Some(repo) = repository().await else {
        return;
    };
    conformance::outbox::worker_claim_complete_and_retry_lifecycle(
        repo.clone(),
        repo.outbox_store(),
    )
    .await;
}

#[tokio::test]
async fn worker_claim_by_ids_claims_only_requested() {
    let Some(repo) = repository().await else {
        return;
    };
    conformance::outbox::worker_claim_by_ids_claims_only_requested(
        repo.clone(),
        repo.outbox_store(),
    )
    .await;
}

#[tokio::test]
async fn worker_completes_claims_in_one_batch() {
    let Some(repo) = repository().await else {
        return;
    };
    conformance::outbox::worker_completes_claims_in_one_batch(repo.clone(), repo.outbox_store())
        .await;
}

#[tokio::test]
async fn consumer_inbox_records_dedupes_and_fences_with_real_effects() {
    let Some(repo) = repository().await else {
        return;
    };
    conformance::inbox::inbox_records_dedupes_and_fences_with_real_effects(
        repo.clone(),
        repo.outbox_store(),
    )
    .await;
}

#[tokio::test]
async fn consumer_inbox_rejects_empty_receipt() {
    let Some(repo) = repository().await else {
        return;
    };
    conformance::inbox::inbox_rejects_empty_receipt(repo).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn racing_commits_one_wins_one_conflicts() {
    let Some(repo) = repository().await else {
        return;
    };
    conformance::scenario::racing_commits_one_wins_one_conflicts(repo, 8).await;
}

#[tokio::test]
async fn expired_outbox_lease_is_reclaimed_by_second_worker() {
    let Some(repo) = repository().await else {
        return;
    };
    conformance::outbox::expired_outbox_lease_is_reclaimed_by_second_worker(
        repo.clone(),
        repo.outbox_store(),
    )
    .await;
}

#[tokio::test]
async fn publish_failure_after_commit_retains_outbox_row_until_delivered() {
    let Some(repo) = repository().await else {
        return;
    };
    conformance::outbox::publish_failure_after_commit_retains_outbox_row_until_delivered(
        repo.clone(),
        repo.outbox_store(),
    )
    .await;
}
