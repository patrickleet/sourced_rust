//! In-memory queue for testing distributed systems.
//!
//! This module provides a thread-safe in-memory queue for testing
//! distributed saga patterns without requiring external dependencies
//! like Kafka or Redis.

#![allow(dead_code)]

use sourced_rust::bus::{Event, PublishError, Publisher, Subscriber};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

/// In-memory queue for testing and single-process scenarios.
///
/// Features:
/// - Thread-safe (can be shared across threads)
/// - Supports multiple subscribers via `new_subscriber()`
/// - Events are stored in an append-only log
/// - Each subscriber tracks its own read position
///
/// ## Example
///
/// ```ignore
/// use support::in_memory_queue::InMemoryQueue;
/// use sourced_rust::bus::{Publisher, Subscriber, Event};
///
/// let queue = InMemoryQueue::new();
///
/// // Publish an event
/// queue.publish(Event::with_string_payload("evt-1", "OrderCreated", r#"{"id":"123"}"#)).unwrap();
///
/// // Subscribe and receive
/// let event = queue.poll(1000).unwrap();
/// assert!(event.is_some());
/// ```
#[derive(Clone)]
pub struct InMemoryQueue {
    /// Shared event log
    log: Arc<RwLock<Vec<Event>>>,
    /// Per-subscriber read position
    position: Arc<Mutex<usize>>,
    /// Acknowledged event IDs
    acked: Arc<Mutex<Vec<String>>>,
}

impl Default for InMemoryQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryQueue {
    /// Create a new in-memory bus.
    pub fn new() -> Self {
        Self {
            log: Arc::new(RwLock::new(Vec::new())),
            position: Arc::new(Mutex::new(0)),
            acked: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Create a new subscriber that shares the same log but has its own position.
    ///
    /// This allows multiple independent consumers of the same event stream.
    pub fn new_subscriber(&self) -> Self {
        Self {
            log: Arc::clone(&self.log),
            position: Arc::new(Mutex::new(0)),
            acked: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Get all events in the log.
    pub fn events(&self) -> Vec<Event> {
        self.log.read().unwrap().clone()
    }

    /// Get all event types in order.
    pub fn event_types(&self) -> Vec<String> {
        self.log
            .read()
            .unwrap()
            .iter()
            .map(|e| e.event_type.clone())
            .collect()
    }

    /// Get the total number of events in the log.
    pub fn len(&self) -> usize {
        self.log.read().unwrap().len()
    }

    /// Check if the log is empty.
    pub fn is_empty(&self) -> bool {
        self.log.read().unwrap().is_empty()
    }

    /// Find an event by type.
    pub fn find_by_type(&self, event_type: &str) -> Option<Event> {
        self.log
            .read()
            .unwrap()
            .iter()
            .find(|e| e.event_type == event_type)
            .cloned()
    }

    /// Find all events matching a type.
    pub fn find_all_by_type(&self, event_type: &str) -> Vec<Event> {
        self.log
            .read()
            .unwrap()
            .iter()
            .filter(|e| e.event_type == event_type)
            .cloned()
            .collect()
    }

    /// Reset the subscriber position to the beginning.
    pub fn reset_position(&self) {
        *self.position.lock().unwrap() = 0;
    }

    /// Get the current subscriber position.
    pub fn current_position(&self) -> usize {
        *self.position.lock().unwrap()
    }
}

impl Publisher for InMemoryQueue {
    fn publish(&self, event: Event) -> Result<(), PublishError> {
        self.log.write().unwrap().push(event);
        Ok(())
    }

    fn publish_batch(&self, events: Vec<Event>) -> Result<(), PublishError> {
        let mut log = self.log.write().unwrap();
        log.extend(events);
        Ok(())
    }
}

impl Subscriber for InMemoryQueue {
    fn poll(&self, timeout_ms: u64) -> Result<Option<Event>, PublishError> {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);

        loop {
            {
                let log = self.log.read().unwrap();
                let mut pos = self.position.lock().unwrap();

                if *pos < log.len() {
                    let event = log[*pos].clone();
                    *pos += 1;
                    return Ok(Some(event));
                }
            }

            if Instant::now() >= deadline {
                return Ok(None);
            }

            // Small sleep to avoid busy-waiting
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn ack(&self, event_id: &str) -> Result<(), PublishError> {
        self.acked.lock().unwrap().push(event_id.to_string());
        Ok(())
    }

    fn nack(&self, _event_id: &str, _reason: &str) -> Result<(), PublishError> {
        // In-memory bus doesn't support redelivery; events stay in log
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publish_and_poll() {
        let bus = InMemoryQueue::new();

        bus.publish(Event::with_string_payload(
            "evt-1",
            "TestEvent",
            r#"{"data": 1}"#,
        ))
        .unwrap();

        let event = bus.poll(100).unwrap();
        assert!(event.is_some());
        let event = event.unwrap();
        assert_eq!(event.event_type, "TestEvent");
        assert_eq!(event.payload_str(), Some(r#"{"data": 1}"#));
    }

    #[test]
    fn poll_timeout_when_empty() {
        let bus = InMemoryQueue::new();
        let event = bus.poll(10).unwrap();
        assert!(event.is_none());
    }

    #[test]
    fn multiple_subscribers() {
        let bus = InMemoryQueue::new();

        bus.publish(Event::with_string_payload("evt-1", "Event1", "{}"))
            .unwrap();
        bus.publish(Event::with_string_payload("evt-2", "Event2", "{}"))
            .unwrap();

        // Create a second subscriber
        let sub2 = bus.new_subscriber();

        // First subscriber reads both events
        assert_eq!(bus.poll(10).unwrap().unwrap().event_type, "Event1");
        assert_eq!(bus.poll(10).unwrap().unwrap().event_type, "Event2");

        // Second subscriber also reads both events (independent position)
        assert_eq!(sub2.poll(10).unwrap().unwrap().event_type, "Event1");
        assert_eq!(sub2.poll(10).unwrap().unwrap().event_type, "Event2");
    }

    #[test]
    fn publish_batch() {
        let bus = InMemoryQueue::new();

        let events = vec![
            Event::with_string_payload("evt-1", "Event1", "{}"),
            Event::with_string_payload("evt-2", "Event2", "{}"),
            Event::with_string_payload("evt-3", "Event3", "{}"),
        ];

        bus.publish_batch(events).unwrap();

        assert_eq!(bus.len(), 3);
        assert_eq!(bus.event_types(), vec!["Event1", "Event2", "Event3"]);
    }

    #[test]
    fn find_by_type() {
        let bus = InMemoryQueue::new();

        bus.publish(Event::with_string_payload(
            "evt-1",
            "OrderCreated",
            r#"{"id":"1"}"#,
        ))
        .unwrap();
        bus.publish(Event::with_string_payload(
            "evt-2",
            "PaymentSucceeded",
            r#"{"id":"2"}"#,
        ))
        .unwrap();
        bus.publish(Event::with_string_payload(
            "evt-3",
            "OrderCreated",
            r#"{"id":"3"}"#,
        ))
        .unwrap();

        let found = bus.find_by_type("PaymentSucceeded");
        assert!(found.is_some());
        assert_eq!(found.unwrap().id, "evt-2");

        let all_orders = bus.find_all_by_type("OrderCreated");
        assert_eq!(all_orders.len(), 2);
    }
}
