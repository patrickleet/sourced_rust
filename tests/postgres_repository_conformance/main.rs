#![cfg(feature = "postgres")]

#[path = "../persistent_repository_conformance/mod.rs"]
mod conformance;

use std::env;

use sourced_rust::PostgresRepository;

async fn repository() -> Option<PostgresRepository> {
    let Ok(database_url) = env::var("DATABASE_URL") else {
        eprintln!("skipping Postgres conformance test: DATABASE_URL is not set");
        return None;
    };

    Some(
        PostgresRepository::connect_and_migrate(&database_url)
            .await
            .expect("postgres conformance repository should migrate"),
    )
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
