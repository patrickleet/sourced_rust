use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use crate::core::{Entity, EventRecord};
use crate::impl_aggregate;

/// Status of an outbox message.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutboxMessageStatus {
    Pending,
    InFlight,
    Published,
    Failed,
}

/// A message in the outbox.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OutboxMessage {
    pub id: u64,
    pub event_type: String,
    pub payload: String,
    pub status: OutboxMessageStatus,
    pub created_at: SystemTime,
    pub attempts: u32,
    pub last_error: Option<String>,
}

/// Event-sourced Outbox for reliable message delivery.
#[derive(Debug, Serialize, Deserialize)]
pub struct Outbox {
    pub entity: Entity,
    #[serde(default)]
    messages: HashMap<u64, OutboxMessage>,
    #[serde(default)]
    next_id: u64,
}

impl Default for Outbox {
    fn default() -> Self {
        Self {
            entity: Entity::new(),
            messages: HashMap::new(),
            next_id: 0,
        }
    }
}

impl Outbox {
    pub fn new(id: impl Into<String>) -> Self {
        let mut outbox = Self::default();
        outbox.entity.set_id(id);
        outbox
    }

    /// Add a message to the outbox.
    pub fn put(&mut self, event_type: impl Into<String>, payload: impl Into<String>) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let event_type = event_type.into();
        let payload = payload.into();

        self.messages.insert(
            id,
            OutboxMessage {
                id,
                event_type: event_type.clone(),
                payload: payload.clone(),
                status: OutboxMessageStatus::Pending,
                created_at: SystemTime::now(),
                attempts: 0,
                last_error: None,
            },
        );

        self.entity
            .digest("MessageCreated", vec![id.to_string(), event_type, payload]);

        id
    }

    /// Claim pending messages for processing.
    pub fn claim(&mut self, worker_id: &str, max: usize, lease: Duration) -> Vec<u64> {
        let now = SystemTime::now();
        let until_secs = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            + lease.as_secs();

        let claimable: Vec<u64> = self
            .messages
            .values()
            .filter(|m| m.status == OutboxMessageStatus::Pending)
            .take(max)
            .map(|m| m.id)
            .collect();

        for &id in &claimable {
            if let Some(msg) = self.messages.get_mut(&id) {
                msg.status = OutboxMessageStatus::InFlight;
                msg.attempts += 1;
            }

            self.entity.digest(
                "MessageClaimed",
                vec![id.to_string(), worker_id.to_string(), until_secs.to_string()],
            );
        }

        claimable
    }

    /// Mark a message as published.
    pub fn complete(&mut self, id: u64) {
        if let Some(msg) = self.messages.get_mut(&id) {
            if msg.status == OutboxMessageStatus::InFlight {
                msg.status = OutboxMessageStatus::Published;
                self.entity
                    .digest("MessagePublished", vec![id.to_string()]);
            }
        }
    }

    /// Release a message back to pending.
    pub fn release(&mut self, id: u64, error: Option<&str>) {
        if let Some(msg) = self.messages.get_mut(&id) {
            if msg.status == OutboxMessageStatus::InFlight {
                msg.status = OutboxMessageStatus::Pending;
                msg.last_error = error.map(String::from);
                self.entity.digest(
                    "MessageReleased",
                    vec![id.to_string(), error.unwrap_or("").to_string()],
                );
            }
        }
    }

    /// Mark a message as permanently failed.
    pub fn fail(&mut self, id: u64, error: Option<&str>) {
        if let Some(msg) = self.messages.get_mut(&id) {
            if msg.status != OutboxMessageStatus::Published
                && msg.status != OutboxMessageStatus::Failed
            {
                msg.status = OutboxMessageStatus::Failed;
                msg.last_error = error.map(String::from);
                self.entity.digest(
                    "MessageFailed",
                    vec![id.to_string(), error.unwrap_or("").to_string()],
                );
            }
        }
    }

    // -- Queries --

    pub fn get(&self, id: u64) -> Option<&OutboxMessage> {
        self.messages.get(&id)
    }

    pub fn pending(&self) -> Vec<&OutboxMessage> {
        self.messages
            .values()
            .filter(|m| m.status == OutboxMessageStatus::Pending)
            .collect()
    }

    pub fn pending_count(&self) -> usize {
        self.messages
            .values()
            .filter(|m| m.status == OutboxMessageStatus::Pending)
            .count()
    }

    pub fn in_flight(&self) -> Vec<&OutboxMessage> {
        self.messages
            .values()
            .filter(|m| m.status == OutboxMessageStatus::InFlight)
            .collect()
    }

    pub fn published(&self) -> Vec<&OutboxMessage> {
        self.messages
            .values()
            .filter(|m| m.status == OutboxMessageStatus::Published)
            .collect()
    }

    pub fn failed(&self) -> Vec<&OutboxMessage> {
        self.messages
            .values()
            .filter(|m| m.status == OutboxMessageStatus::Failed)
            .collect()
    }

    pub fn len(&self) -> usize {
        self.messages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    // -- Replay (called by repository during hydration) --

    fn replay(&mut self, event: &EventRecord) -> Result<(), String> {
        match event.event_name.as_str() {
            "MessageCreated" => {
                let (id_str, event_type, payload) = event.args3().map_err(|e| e.to_string())?;
                let id: u64 = id_str
                    .parse()
                    .map_err(|e: std::num::ParseIntError| e.to_string())?;

                self.messages.insert(
                    id,
                    OutboxMessage {
                        id,
                        event_type: event_type.to_string(),
                        payload: payload.to_string(),
                        status: OutboxMessageStatus::Pending,
                        created_at: event.timestamp,
                        attempts: 0,
                        last_error: None,
                    },
                );
                self.next_id = self.next_id.max(id + 1);
            }
            "MessageClaimed" => {
                let (id_str, _worker_id, _until) = event.args3().map_err(|e| e.to_string())?;
                let id: u64 = id_str
                    .parse()
                    .map_err(|e: std::num::ParseIntError| e.to_string())?;

                if let Some(msg) = self.messages.get_mut(&id) {
                    msg.status = OutboxMessageStatus::InFlight;
                    msg.attempts += 1;
                }
            }
            "MessagePublished" => {
                let id_str = event.arg(0).ok_or("missing id")?;
                let id: u64 = id_str
                    .parse()
                    .map_err(|e: std::num::ParseIntError| e.to_string())?;

                if let Some(msg) = self.messages.get_mut(&id) {
                    msg.status = OutboxMessageStatus::Published;
                }
            }
            "MessageReleased" => {
                let (id_str, error) = event.args2().map_err(|e| e.to_string())?;
                let id: u64 = id_str
                    .parse()
                    .map_err(|e: std::num::ParseIntError| e.to_string())?;

                if let Some(msg) = self.messages.get_mut(&id) {
                    msg.status = OutboxMessageStatus::Pending;
                    if !error.is_empty() {
                        msg.last_error = Some(error.to_string());
                    }
                }
            }
            "MessageFailed" => {
                let (id_str, error) = event.args2().map_err(|e| e.to_string())?;
                let id: u64 = id_str
                    .parse()
                    .map_err(|e: std::num::ParseIntError| e.to_string())?;

                if let Some(msg) = self.messages.get_mut(&id) {
                    msg.status = OutboxMessageStatus::Failed;
                    if !error.is_empty() {
                        msg.last_error = Some(error.to_string());
                    }
                }
            }
            _ => return Err(format!("Unknown event: {}", event.event_name)),
        }
        Ok(())
    }
}

impl_aggregate!(Outbox, entity, replay);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_message() {
        let mut outbox = Outbox::new("test");
        let id = outbox.put("UserCreated", r#"{"id":"123"}"#);

        assert_eq!(id, 0);
        assert_eq!(outbox.len(), 1);
        assert_eq!(outbox.pending_count(), 1);

        let msg = outbox.get(0).unwrap();
        assert_eq!(msg.event_type, "UserCreated");
    }

    #[test]
    fn claim_and_complete() {
        let mut outbox = Outbox::new("test");
        outbox.put("Event1", "{}");
        outbox.put("Event2", "{}");

        let claimed = outbox.claim("worker-1", 1, Duration::from_secs(60));
        assert_eq!(claimed.len(), 1);
        assert_eq!(outbox.in_flight().len(), 1);
        assert_eq!(outbox.pending_count(), 1);

        outbox.complete(claimed[0]);
        assert_eq!(outbox.published().len(), 1);
    }

    #[test]
    fn release_and_fail() {
        let mut outbox = Outbox::new("test");
        outbox.put("Event1", "{}");

        let claimed = outbox.claim("worker-1", 1, Duration::from_secs(60));
        outbox.release(claimed[0], Some("timeout"));

        let msg = outbox.get(claimed[0]).unwrap();
        assert_eq!(msg.status, OutboxMessageStatus::Pending);
        assert_eq!(msg.last_error.as_deref(), Some("timeout"));

        outbox.claim("worker-1", 1, Duration::from_secs(60));
        outbox.fail(claimed[0], Some("max retries"));

        let msg = outbox.get(claimed[0]).unwrap();
        assert_eq!(msg.status, OutboxMessageStatus::Failed);
    }
}
