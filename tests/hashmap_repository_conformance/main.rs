#[path = "../persistent_repository_conformance/mod.rs"]
mod conformance;

use sourced_rust::HashMapRepository;

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
