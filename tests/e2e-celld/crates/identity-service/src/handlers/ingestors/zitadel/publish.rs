//! Shared outbox publish for provider messages (ingress + scrape).

use distributed::{CommitBuilderExt, OutboxMessage, TransactionalCommit};

use super::map::MappedDelivery;

/// Encode + leaf-outbox commit a mapped provider delivery.
pub async fn publish_mapped_delivery<R: TransactionalCommit>(
    repo: &R,
    mapped: &MappedDelivery,
) -> Result<(), String> {
    let outbox = OutboxMessage::encode(
        mapped.delivery_id.clone(),
        mapped.message_name.as_str(),
        &mapped.payload,
    )
    .map_err(|e| e.to_string())?;
    CommitBuilderExt::outbox(repo, outbox)
        .commit_all()
        .await
        .map_err(|e| e.to_string())
}
