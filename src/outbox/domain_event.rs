use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use crate::core::{Entity, EventRecord};
use crate::impl_aggregate;

/// Status of a domain event message.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DomainEventStatus {
    Pending,
    InFlight,
    Published,
    Failed,
}

/// Event-sourced domain event message.
#[derive(Debug, Serialize, Deserialize)]
pub struct DomainEvent {
    pub entity: Entity,
    pub event_type: String,
    pub payload: String,
    pub status: DomainEventStatus,
    pub created_at: SystemTime,
    pub attempts: u32,
    pub last_error: Option<String>,
    pub worker_id: Option<String>,
    pub leased_until: Option<SystemTime>,
}

impl Default for DomainEvent {
    fn default() -> Self {
        Self {
            entity: Entity::new(),
            event_type: String::new(),
            payload: String::new(),
            status: DomainEventStatus::Pending,
            created_at: SystemTime::now(),
            attempts: 0,
            last_error: None,
            worker_id: None,
            leased_until: None,
        }
    }
}

impl DomainEvent {
    pub const ID_PREFIX: &'static str = "outbox:";

    pub fn new(
        id: impl Into<String>,
        event_type: impl Into<String>,
        payload: impl Into<String>,
    ) -> Self {
        let event_type = event_type.into();
        let payload = payload.into();
        let id = Self::normalize_id(id);

        let mut message = Self {
            entity: Entity::with_id(id),
            event_type: event_type.clone(),
            payload: payload.clone(),
            status: DomainEventStatus::Pending,
            created_at: SystemTime::now(),
            attempts: 0,
            last_error: None,
            worker_id: None,
            leased_until: None,
        };

        message
            .entity
            .digest("MessageCreated", vec![event_type, payload]);

        message
    }

    pub fn id(&self) -> &str {
        self.entity.id()
    }

    pub fn is_pending(&self) -> bool {
        self.status == DomainEventStatus::Pending
    }

    pub fn is_in_flight(&self) -> bool {
        self.status == DomainEventStatus::InFlight
    }

    pub fn is_published(&self) -> bool {
        self.status == DomainEventStatus::Published
    }

    pub fn is_failed(&self) -> bool {
        self.status == DomainEventStatus::Failed
    }

    pub fn claim(&mut self, worker_id: &str, lease: Duration) {
        if self.status != DomainEventStatus::Pending {
            return;
        }

        let now = SystemTime::now();
        let until = now + lease;
        let until_secs = until
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        self.status = DomainEventStatus::InFlight;
        self.attempts += 1;
        self.worker_id = Some(worker_id.to_string());
        self.leased_until = Some(until);

        self.entity.digest(
            "MessageClaimed",
            vec![worker_id.to_string(), until_secs.to_string()],
        );
    }

    pub fn complete(&mut self) {
        if self.status == DomainEventStatus::InFlight {
            self.status = DomainEventStatus::Published;
            self.entity.digest("MessagePublished", Vec::new());
        }
    }

    pub fn release(&mut self, error: Option<&str>) {
        if self.status == DomainEventStatus::InFlight {
            self.status = DomainEventStatus::Pending;
            self.last_error = error.map(String::from);
            self.worker_id = None;
            self.leased_until = None;
            self.entity
                .digest("MessageReleased", vec![error.unwrap_or("").to_string()]);
        }
    }

    pub fn fail(&mut self, error: Option<&str>) {
        if self.status != DomainEventStatus::Published
            && self.status != DomainEventStatus::Failed
        {
            self.status = DomainEventStatus::Failed;
            self.last_error = error.map(String::from);
            self.worker_id = None;
            self.leased_until = None;
            self.entity
                .digest("MessageFailed", vec![error.unwrap_or("").to_string()]);
        }
    }

    fn normalize_id(id: impl Into<String>) -> String {
        let id = id.into();
        if id.starts_with(Self::ID_PREFIX) {
            id
        } else {
            format!("{}{}", Self::ID_PREFIX, id)
        }
    }

    fn replay(&mut self, event: &EventRecord) -> Result<(), String> {
        match event.event_name.as_str() {
            "MessageCreated" => {
                let (event_type, payload) = event.args2().map_err(|e| e.to_string())?;
                self.event_type = event_type.to_string();
                self.payload = payload.to_string();
                self.status = DomainEventStatus::Pending;
                self.created_at = event.timestamp;
            }
            "MessageClaimed" => {
                let (worker_id, until) = event.args2().map_err(|e| e.to_string())?;
                let until_secs: u64 = until
                    .parse()
                    .map_err(|e: std::num::ParseIntError| e.to_string())?;
                let until_time = SystemTime::UNIX_EPOCH + Duration::from_secs(until_secs);

                self.status = DomainEventStatus::InFlight;
                self.attempts += 1;
                self.worker_id = Some(worker_id.to_string());
                self.leased_until = Some(until_time);
            }
            "MessagePublished" => {
                self.status = DomainEventStatus::Published;
                self.worker_id = None;
                self.leased_until = None;
            }
            "MessageReleased" => {
                let error = event.arg(0).unwrap_or("");
                self.status = DomainEventStatus::Pending;
                self.worker_id = None;
                self.leased_until = None;
                self.last_error = if error.is_empty() {
                    None
                } else {
                    Some(error.to_string())
                };
            }
            "MessageFailed" => {
                let error = event.arg(0).unwrap_or("");
                self.status = DomainEventStatus::Failed;
                self.worker_id = None;
                self.leased_until = None;
                self.last_error = if error.is_empty() {
                    None
                } else {
                    Some(error.to_string())
                };
            }
            _ => return Err(format!("Unknown event: {}", event.event_name)),
        }
        Ok(())
    }
}

impl_aggregate!(DomainEvent, entity, replay);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_message_is_pending() {
        let message = DomainEvent::new("msg-1", "UserCreated", r#"{"id":"123"}"#);
        assert_eq!(message.event_type, "UserCreated");
        assert!(message.is_pending());
    }

    #[test]
    fn claim_and_complete() {
        let mut message = DomainEvent::new("msg-1", "Event1", "{}");
        message.claim("worker-1", Duration::from_secs(60));
        assert!(message.is_in_flight());
        assert_eq!(message.attempts, 1);

        message.complete();
        assert!(message.is_published());
    }

    #[test]
    fn release_and_fail() {
        let mut message = DomainEvent::new("msg-1", "Event1", "{}");
        message.claim("worker-1", Duration::from_secs(60));

        message.release(Some("timeout"));
        assert!(message.is_pending());
        assert_eq!(message.last_error.as_deref(), Some("timeout"));

        message.claim("worker-1", Duration::from_secs(60));
        message.fail(Some("max retries"));
        assert!(message.is_failed());
    }
}
