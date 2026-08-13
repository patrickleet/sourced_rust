use distributed::{
    AggregateRepository, InMemoryLockManager, InMemoryRepository, OutboxMessage, QueuedRepository,
    SourcedResult,
};
use serde::Serialize;

use crate::models::counter::Counter;

pub type Repo =
    AggregateRepository<QueuedRepository<InMemoryRepository, InMemoryLockManager>, Counter>;

#[derive(Serialize)]
struct CounterState<'a> {
    id: &'a str,
    value: i64,
}

fn counter_state_message(counter: &Counter, event_type: &str) -> SourcedResult<OutboxMessage> {
    OutboxMessage::encode_for_entity(
        format!(
            "{}:{event_type}:{}",
            counter.entity.id(),
            counter.entity.version()
        ),
        event_type,
        &CounterState {
            id: counter.entity.id(),
            value: counter.value,
        },
        &counter.entity,
    )
}

pub mod counter_create;
pub mod counter_increment;
pub mod whoami;
