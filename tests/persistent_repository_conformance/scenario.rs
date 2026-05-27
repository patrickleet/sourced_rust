use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use sourced_rust::{
    Aggregate, AsyncAggregateBuilder, AsyncCommitBatch, AsyncGetStream, AsyncSnapshotStore,
    AsyncSnapshotWrite, AsyncStreamWrite, AsyncTransactionalCommit, Entity, RepositoryError,
    SnapshotRecord, StreamIdentity,
};

use super::checkout::{CHECKOUT_SEAT_RESERVED_STATUS, SEAT_RESERVED_STATUS};
use super::checkout_saga::CheckoutSaga;
use super::seat::Seat;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub fn unique_id(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after UNIX epoch")
        .as_nanos();
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{nanos}-{id}")
}

pub async fn aggregate_checkout_flow_persists_reloaded_state<R>(repo: R)
where
    R: AsyncGetStream + AsyncTransactionalCommit + Clone + Send + Sync + 'static,
{
    let checkout_id = unique_id("checkout");
    let seat_id = unique_id("seat");
    let seat_category = "balcony".to_string();

    let seat_added = add_seat(repo.clone(), seat_id.clone(), seat_category.clone())
        .await
        .expect("seat add should commit");
    assert_eq!(seat_added.seat_id, seat_id);

    let checkout_started = start_checkout(
        repo.clone(),
        checkout_id.clone(),
        seat_id.clone(),
        seat_category.clone(),
    )
    .await
    .expect("checkout start should commit");

    let seat_reserved = reserve_started_checkout_seat(repo.clone(), checkout_started)
        .await
        .expect("seat reservation should commit");
    assert_eq!(seat_reserved.checkout_id, checkout_id);

    let completed = record_seat_reserved(repo.clone(), seat_reserved)
        .await
        .expect("checkout saga should record reservation");
    assert_eq!(completed.seat_id, seat_id);

    let checkout_repo = repo.clone().async_aggregate::<CheckoutSaga>();
    let loaded_checkout = checkout_repo
        .get(&checkout_id)
        .await
        .expect("checkout reload should succeed")
        .expect("checkout should exist");
    assert_eq!(loaded_checkout.status, CHECKOUT_SEAT_RESERVED_STATUS);
    assert_eq!(loaded_checkout.reserved_seat_id, seat_id);
    assert_eq!(loaded_checkout.entity.events().len(), 2);

    let seat_repo = repo.async_aggregate::<Seat>();
    let loaded_seat = seat_repo
        .get(&seat_id)
        .await
        .expect("seat reload should succeed")
        .expect("seat should exist");
    assert_eq!(loaded_seat.status, SEAT_RESERVED_STATUS);
    assert_eq!(loaded_seat.checkout_id, checkout_id);
    assert_eq!(loaded_seat.entity.events().len(), 2);
}

pub async fn get_all_and_commit_all_round_trip<R>(repo: R)
where
    R: AsyncGetStream + AsyncTransactionalCommit + Clone + Send + Sync + 'static,
{
    let first_id = unique_id("seat-a");
    let second_id = unique_id("seat-b");
    let seat_repo = repo.async_aggregate::<Seat>();

    let mut first = Seat::default();
    first
        .add(first_id.clone(), "floor".into())
        .expect("first seat should be valid");
    let mut second = Seat::default();
    second
        .add(second_id.clone(), "box".into())
        .expect("second seat should be valid");

    seat_repo
        .commit_all(&mut [&mut first, &mut second])
        .await
        .expect("commit_all should persist both seats");

    let loaded = seat_repo
        .get_all(&[first_id.as_str(), second_id.as_str()])
        .await
        .expect("get_all should reload both seats");
    assert_eq!(loaded.len(), 2);
    let mut actual_ids = loaded
        .iter()
        .map(|seat| seat.entity.id().to_string())
        .collect::<Vec<_>>();
    actual_ids.sort();
    let mut expected_ids = vec![first_id, second_id];
    expected_ids.sort();
    assert_eq!(actual_ids, expected_ids);
}

pub async fn multi_stream_conflict_rolls_back_other_stream_and_snapshot<R>(repo: R)
where
    R: AsyncGetStream
        + AsyncTransactionalCommit
        + AsyncSnapshotStore
        + Clone
        + Send
        + Sync
        + 'static,
{
    let seat_repo = repo.clone().async_aggregate::<Seat>();
    let seat_id = unique_id("conflict-seat");

    let mut original = Seat::default();
    original
        .add(seat_id.clone(), "balcony".into())
        .expect("seat should be valid");
    seat_repo
        .commit(&mut original)
        .await
        .expect("initial seat commit should succeed");

    let mut stale = seat_repo
        .get(&seat_id)
        .await
        .expect("stale load should succeed")
        .expect("stale seat should exist");
    let mut winner = seat_repo
        .get(&seat_id)
        .await
        .expect("winner load should succeed")
        .expect("winner seat should exist");
    stale
        .reserve(
            unique_id("checkout-stale"),
            seat_id.clone(),
            stale.category.clone(),
        )
        .expect("stale reservation should be valid locally");
    winner
        .reserve(
            unique_id("checkout-winner"),
            seat_id.clone(),
            winner.category.clone(),
        )
        .expect("winner reservation should be valid locally");
    seat_repo
        .commit(&mut winner)
        .await
        .expect("winner commit should succeed");

    let checkout_id = unique_id("rollback-checkout");
    let mut checkout = CheckoutSaga::default();
    checkout
        .start(
            checkout_id.clone(),
            unique_id("rollback-seat"),
            "floor".into(),
        )
        .expect("checkout should be valid locally");

    let stale_identity = StreamIdentity::new(Seat::aggregate_type(), &seat_id)
        .expect("stale identity should be valid");
    let checkout_identity = StreamIdentity::new(CheckoutSaga::aggregate_type(), &checkout_id)
        .expect("checkout identity should be valid");
    let err = repo
        .commit_batch_async(AsyncCommitBatch {
            streams: vec![
                AsyncStreamWrite::new(stale_identity, stale.entity_mut()),
                AsyncStreamWrite::new(checkout_identity.clone(), checkout.entity_mut()),
            ],
            outbox_messages: Vec::new(),
            read_model_plans: Vec::new(),
            snapshots: vec![AsyncSnapshotWrite::Save {
                identity: checkout_identity.clone(),
                record: SnapshotRecord::new(
                    CheckoutSaga::aggregate_type(),
                    checkout_id,
                    1,
                    "CheckoutSnapshot",
                    1,
                    vec![7],
                ),
            }],
        })
        .await
        .expect_err("stale batch should conflict");

    assert!(matches!(err, RepositoryError::ConcurrentWrite { .. }));
    assert!(repo
        .get_stream(&checkout_identity)
        .await
        .expect("rollback stream lookup should succeed")
        .is_none());
    assert!(repo
        .get_snapshot_async(&checkout_identity)
        .await
        .expect("rollback snapshot lookup should succeed")
        .is_none());
    assert_eq!(stale.entity.committed_version(), 1);
    assert_eq!(stale.entity.new_events().len(), 1);
}

pub async fn duplicate_stream_identity_is_rejected_before_write<R>(repo: R)
where
    R: AsyncGetStream + AsyncTransactionalCommit + Send + Sync + 'static,
{
    let id = unique_id("duplicate-seat");
    let identity =
        StreamIdentity::new(Seat::aggregate_type(), &id).expect("identity should be valid");
    let mut first = Entity::with_id(&id);
    first
        .digest_empty("First")
        .expect("first event should encode");
    let mut second = Entity::with_id(&id);
    second
        .digest_empty("Second")
        .expect("second event should encode");

    let err = repo
        .commit_batch_async(AsyncCommitBatch::new(vec![
            AsyncStreamWrite::new(identity.clone(), &mut first),
            AsyncStreamWrite::new(identity.clone(), &mut second),
        ]))
        .await
        .expect_err("duplicate stream should be rejected");

    assert_eq!(
        err,
        RepositoryError::DuplicateStreamInBatch {
            id: format!("{}:{id}", Seat::aggregate_type())
        }
    );
    assert!(repo
        .get_stream(&identity)
        .await
        .expect("duplicate stream lookup should succeed")
        .is_none());
}

pub async fn metadata_round_trips<R>(repo: R)
where
    R: AsyncGetStream + AsyncTransactionalCommit + Clone + Send + Sync + 'static,
{
    let id = unique_id("metadata-seat");
    let mut seat = Seat::default();
    seat.entity.set_correlation_id("corr-conformance");
    seat.entity.set_causation_id("cmd-conformance");
    seat.add(id.clone(), "gallery".into())
        .expect("seat should be valid");

    repo.clone()
        .async_aggregate::<Seat>()
        .commit(&mut seat)
        .await
        .expect("metadata seat should commit");

    let identity =
        StreamIdentity::new(Seat::aggregate_type(), &id).expect("identity should be valid");
    let entity = repo
        .get_stream(&identity)
        .await
        .expect("metadata stream should reload")
        .expect("metadata stream should exist");
    let event = entity
        .events()
        .first()
        .expect("metadata stream should contain one event");
    assert_eq!(event.correlation_id(), Some("corr-conformance"));
    assert_eq!(event.causation_id(), Some("cmd-conformance"));
}

pub async fn unsupported_codec_is_rejected_on_write<R>(repo: R)
where
    R: AsyncGetStream + AsyncTransactionalCommit + Send + Sync + 'static,
{
    let id = unique_id("bad-codec-seat");
    let identity =
        StreamIdentity::new(Seat::aggregate_type(), &id).expect("identity should be valid");
    let mut entity = Entity::with_id(&id);
    entity
        .digest_empty("BadCodec")
        .expect("event should encode before codec mutation");

    let mut value = serde_json::to_value(&entity).expect("entity should serialize");
    value["events"][0]["payload_codec"] = serde_json::json!("json");
    let mut bad_entity: Entity =
        serde_json::from_value(value).expect("mutated entity should deserialize");

    let err = repo
        .commit_batch_async(AsyncCommitBatch::new(vec![AsyncStreamWrite::new(
            identity.clone(),
            &mut bad_entity,
        )]))
        .await
        .expect_err("unsupported codec should be rejected");
    assert!(
        matches!(err, RepositoryError::Model(message) if message.contains("unsupported payload codec"))
    );
    assert!(repo
        .get_stream(&identity)
        .await
        .expect("bad codec stream lookup should succeed")
        .is_none());
}

pub async fn snapshots_use_full_stream_identity<R>(repo: R)
where
    R: AsyncSnapshotStore + Send + Sync + 'static,
{
    let id = unique_id("snapshot");
    let seat_identity =
        StreamIdentity::new(Seat::aggregate_type(), &id).expect("seat identity should be valid");
    let checkout_identity = StreamIdentity::new(CheckoutSaga::aggregate_type(), &id)
        .expect("checkout identity should be valid");

    repo.save_snapshot_async(
        &seat_identity,
        SnapshotRecord::new(
            Seat::aggregate_type(),
            id.clone(),
            1,
            "SeatSnapshot",
            1,
            vec![1],
        ),
    )
    .await
    .expect("seat snapshot should save");
    repo.save_snapshot_async(
        &checkout_identity,
        SnapshotRecord::new(
            CheckoutSaga::aggregate_type(),
            id,
            2,
            "CheckoutSnapshot",
            1,
            vec![2],
        ),
    )
    .await
    .expect("checkout snapshot should save");

    let loaded_seat = repo
        .get_snapshot_async(&seat_identity)
        .await
        .expect("seat snapshot should load")
        .expect("seat snapshot should exist");
    let loaded_checkout = repo
        .get_snapshot_async(&checkout_identity)
        .await
        .expect("checkout snapshot should load")
        .expect("checkout snapshot should exist");

    assert_eq!(loaded_seat.version, 1);
    assert_eq!(loaded_seat.aggregate_type, Seat::aggregate_type());
    assert_eq!(loaded_seat.snapshot_type, "SeatSnapshot");
    assert_eq!(loaded_seat.payload, vec![1]);
    assert_eq!(loaded_checkout.version, 2);
    assert_eq!(
        loaded_checkout.aggregate_type,
        CheckoutSaga::aggregate_type()
    );
    assert_eq!(loaded_checkout.snapshot_type, "CheckoutSnapshot");
    assert_eq!(loaded_checkout.payload, vec![2]);
}

async fn add_seat<R>(
    repo: R,
    seat_id: String,
    category: String,
) -> Result<super::checkout::SeatAdded, RepositoryError>
where
    R: AsyncTransactionalCommit + Clone + Send + Sync + 'static,
{
    let mut seat = Seat::default();
    seat.add(seat_id.clone(), category.clone())
        .map_err(|err| RepositoryError::Model(err.to_string()))?;
    repo.async_aggregate::<Seat>().commit(&mut seat).await?;
    Ok(super::checkout::SeatAdded { seat_id, category })
}

async fn start_checkout<R>(
    repo: R,
    checkout_id: String,
    seat_id: String,
    seat_category: String,
) -> Result<super::checkout::CheckoutStarted, RepositoryError>
where
    R: AsyncTransactionalCommit + Clone + Send + Sync + 'static,
{
    let mut checkout = CheckoutSaga::default();
    checkout
        .start(checkout_id.clone(), seat_id.clone(), seat_category.clone())
        .map_err(|err| RepositoryError::Model(err.to_string()))?;
    repo.async_aggregate::<CheckoutSaga>()
        .commit(&mut checkout)
        .await?;
    Ok(super::checkout::CheckoutStarted {
        checkout_id,
        seat_id,
        seat_category,
    })
}

async fn reserve_started_checkout_seat<R>(
    repo: R,
    event: super::checkout::CheckoutStarted,
) -> Result<super::checkout::SeatReserved, RepositoryError>
where
    R: AsyncGetStream + AsyncTransactionalCommit + Clone + Send + Sync + 'static,
{
    let seat_repo = repo.async_aggregate::<Seat>();
    let mut seat =
        seat_repo
            .get(&event.seat_id)
            .await?
            .ok_or_else(|| RepositoryError::NotFound {
                id: event.seat_id.clone(),
            })?;
    let reserved = super::checkout::SeatReserved {
        checkout_id: event.checkout_id,
        seat_id: event.seat_id.clone(),
        seat_category: event.seat_category.clone(),
    };
    seat.reserve(
        reserved.checkout_id.clone(),
        reserved.seat_id.clone(),
        reserved.seat_category.clone(),
    )
    .map_err(|err| RepositoryError::Model(err.to_string()))?;
    seat_repo.commit(&mut seat).await?;
    Ok(reserved)
}

async fn record_seat_reserved<R>(
    repo: R,
    event: super::checkout::SeatReserved,
) -> Result<super::checkout::SeatReservationCompleted, RepositoryError>
where
    R: AsyncGetStream + AsyncTransactionalCommit + Clone + Send + Sync + 'static,
{
    let checkout_repo = repo.async_aggregate::<CheckoutSaga>();
    let mut checkout = checkout_repo
        .get(&event.checkout_id)
        .await?
        .ok_or_else(|| RepositoryError::NotFound {
            id: event.checkout_id.clone(),
        })?;
    checkout
        .record_seat_reserved(
            event.checkout_id.clone(),
            event.seat_id.clone(),
            event.seat_category.clone(),
        )
        .map_err(|err| RepositoryError::Model(err.to_string()))?;
    checkout_repo.commit(&mut checkout).await?;
    Ok(super::checkout::SeatReservationCompleted {
        checkout_id: event.checkout_id,
        seat_id: event.seat_id,
        seat_category: event.seat_category,
    })
}
