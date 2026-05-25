use std::time::Duration;

use sourced_rust::{
    Aggregate, AsyncAggregateBuilder, AsyncGetStream, AsyncOutboxRepositoryExt,
    AsyncTransactionalCommit, OutboxMessage, OutboxMessageStatus, OutboxPublishFailureAction,
    RepositoryError, StreamIdentity,
};

use super::scenario::unique_id;
use super::seat::Seat;

pub async fn high_level_outbox_commit_persists_row_without_stream<R>(repo: R)
where
    R: AsyncGetStream
        + AsyncTransactionalCommit
        + AsyncOutboxRepositoryExt
        + Clone
        + Send
        + Sync
        + 'static,
{
    let seat_id = unique_id("outbox-seat");
    let message_id = unique_id("outbox-message");
    let mut seat = added_seat(&seat_id);
    let message = OutboxMessage::create(&message_id, "SeatAdded", b"{}".to_vec())
        .expect("outbox message should be valid");

    repo.clone()
        .async_aggregate::<Seat>()
        .outbox(message)
        .commit(&mut seat)
        .await
        .expect("aggregate and outbox should commit atomically");

    let stored = find_outbox_by_id(&repo, &message_id)
        .await
        .expect("outbox message should be stored");
    assert_eq!(stored.status, OutboxMessageStatus::Pending);
    assert_eq!(
        stored.source_aggregate_type.as_deref(),
        Some(Seat::aggregate_type())
    );
    assert_eq!(
        stored.source_aggregate_id.as_deref(),
        Some(seat_id.as_str())
    );
    assert_eq!(stored.source_sequence, Some(1));

    let old_outbox_stream =
        StreamIdentity::new("sourced_rust::outbox::message::OutboxMessage", stored.id())
            .expect("old outbox stream identity should be syntactically valid");
    assert!(repo
        .get_stream(&old_outbox_stream)
        .await
        .expect("old outbox stream lookup should succeed")
        .is_none());
}

pub async fn duplicate_outbox_insert_rolls_back_aggregate<R>(repo: R)
where
    R: AsyncGetStream
        + AsyncTransactionalCommit
        + AsyncOutboxRepositoryExt
        + Clone
        + Send
        + Sync
        + 'static,
{
    let duplicate_message_id = unique_id("duplicate-outbox");
    let mut existing_seat = added_seat(&unique_id("existing-seat"));
    let existing_message =
        OutboxMessage::create(&duplicate_message_id, "SeatAdded", b"{}".to_vec())
            .expect("existing outbox message should be valid");
    repo.clone()
        .async_aggregate::<Seat>()
        .outbox(existing_message)
        .commit(&mut existing_seat)
        .await
        .expect("initial outbox commit should succeed");

    let rollback_seat_id = unique_id("rollback-seat");
    let rollback_identity = StreamIdentity::new(Seat::aggregate_type(), &rollback_seat_id)
        .expect("seat stream identity should be valid");
    let mut rollback_seat = added_seat(&rollback_seat_id);
    let duplicate_message =
        OutboxMessage::create(&duplicate_message_id, "SeatAddedAgain", b"{}".to_vec())
            .expect("duplicate outbox message should be valid");

    let err = repo
        .clone()
        .async_aggregate::<Seat>()
        .outbox(duplicate_message)
        .commit(&mut rollback_seat)
        .await
        .expect_err("duplicate outbox id should reject the batch");

    assert!(matches!(
        err,
        RepositoryError::DuplicateOutboxMessageInBatch { .. }
    ));
    assert!(repo
        .get_stream(&rollback_identity)
        .await
        .expect("rollback stream lookup should succeed")
        .is_none());
    assert_eq!(rollback_seat.entity.committed_version(), 0);
}

pub async fn aggregate_conflict_rolls_back_outbox<R>(repo: R)
where
    R: AsyncGetStream
        + AsyncTransactionalCommit
        + AsyncOutboxRepositoryExt
        + Clone
        + Send
        + Sync
        + 'static,
{
    let seat_id = unique_id("conflict-outbox-seat");
    let seat_repo = repo.clone().async_aggregate::<Seat>();
    let mut original = added_seat(&seat_id);
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
            unique_id("stale-checkout"),
            seat_id.clone(),
            stale.category.clone(),
        )
        .expect("stale reservation should be valid locally");
    winner
        .reserve(
            unique_id("winner-checkout"),
            seat_id.clone(),
            winner.category.clone(),
        )
        .expect("winner reservation should be valid locally");
    seat_repo
        .commit(&mut winner)
        .await
        .expect("winner commit should succeed");

    let message_id = unique_id("rollback-outbox-message");
    let message = OutboxMessage::create(&message_id, "SeatReserved", b"{}".to_vec())
        .expect("outbox message should be valid");
    let err = repo
        .clone()
        .async_aggregate::<Seat>()
        .outbox(message)
        .commit(&mut stale)
        .await
        .expect_err("stale aggregate should reject the batch");

    assert!(matches!(err, RepositoryError::ConcurrentWrite { .. }));
    assert!(find_outbox_by_id(&repo, &message_id).await.is_none());
}

pub async fn worker_claim_complete_and_retry_lifecycle<R>(repo: R)
where
    R: AsyncGetStream
        + AsyncTransactionalCommit
        + AsyncOutboxRepositoryExt
        + Clone
        + Send
        + Sync
        + 'static,
{
    let complete_message_id = unique_id("complete-outbox");
    let mut complete_seat = added_seat(&unique_id("complete-seat"));
    let complete_message = OutboxMessage::create(&complete_message_id, "SeatAdded", b"{}".to_vec())
        .expect("complete outbox message should be valid");
    repo.clone()
        .async_aggregate::<Seat>()
        .outbox(complete_message)
        .commit(&mut complete_seat)
        .await
        .expect("complete message should be stored");

    let claimed = repo
        .claim_outbox_messages_async("worker-a", 1, Duration::from_secs(60))
        .await
        .expect("claim should succeed");
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].id(), complete_message_id);

    let stale_err = repo
        .complete_outbox_message_for_worker_async(claimed[0].id(), "worker-b")
        .await
        .expect_err("wrong worker should not complete a claim");
    assert!(matches!(stale_err, RepositoryError::InvalidState { .. }));

    repo.complete_outbox_message_for_worker_async(claimed[0].id(), "worker-a")
        .await
        .expect("owning worker should complete the claim");
    let published = find_outbox_by_id(&repo, &complete_message_id)
        .await
        .expect("completed message should still be queryable");
    assert_eq!(published.status, OutboxMessageStatus::Published);

    let retry_message_id = unique_id("retry-outbox");
    let mut retry_seat = added_seat(&unique_id("retry-seat"));
    let retry_message = OutboxMessage::create(&retry_message_id, "SeatAdded", b"{}".to_vec())
        .expect("retry outbox message should be valid");
    repo.clone()
        .async_aggregate::<Seat>()
        .outbox(retry_message)
        .commit(&mut retry_seat)
        .await
        .expect("retry message should be stored");

    let claimed = repo
        .claim_outbox_messages_async("worker-r", 1, Duration::from_secs(60))
        .await
        .expect("retry claim should succeed");
    let action = repo
        .record_outbox_publish_failure_async(claimed[0].id(), "worker-r", "first failure", 2)
        .await
        .expect("first failure should be recorded");
    assert_eq!(action, OutboxPublishFailureAction::Released);

    let released = find_outbox_by_id(&repo, &retry_message_id)
        .await
        .expect("released message should exist");
    assert_eq!(released.status, OutboxMessageStatus::Pending);
    assert_eq!(released.attempts, 1);
    assert_eq!(released.last_error.as_deref(), Some("first failure"));

    let claimed = repo
        .claim_outbox_messages_async("worker-r", 1, Duration::from_secs(60))
        .await
        .expect("second retry claim should succeed");
    let action = repo
        .record_outbox_publish_failure_async(claimed[0].id(), "worker-r", "second failure", 2)
        .await
        .expect("second failure should be recorded");
    assert_eq!(action, OutboxPublishFailureAction::Failed);

    let failed = find_outbox_by_id(&repo, &retry_message_id)
        .await
        .expect("failed message should exist");
    assert_eq!(failed.status, OutboxMessageStatus::Failed);
    assert_eq!(failed.attempts, 2);
    assert_eq!(failed.last_error.as_deref(), Some("second failure"));
}

fn added_seat(id: &str) -> Seat {
    let mut seat = Seat::default();
    seat.add(id.to_string(), "floor".to_string())
        .expect("seat should be valid");
    seat
}

async fn find_outbox_by_id<R>(repo: &R, id: &str) -> Option<OutboxMessage>
where
    R: AsyncOutboxRepositoryExt + Send + Sync,
{
    for status in [
        OutboxMessageStatus::Pending,
        OutboxMessageStatus::InFlight,
        OutboxMessageStatus::Published,
        OutboxMessageStatus::Failed,
    ] {
        let messages = repo
            .outbox_messages_by_status_async(status)
            .await
            .expect("outbox status lookup should succeed");
        if let Some(message) = messages.into_iter().find(|message| message.id() == id) {
            return Some(message);
        }
    }
    None
}
