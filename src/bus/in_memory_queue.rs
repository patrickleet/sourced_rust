//! In-memory queue for testing and single-process scenarios.
//!
//! This module provides a thread-safe in-memory queue that implements
//! both `Publisher` and `Subscriber` traits, useful for:
//! - Unit and integration testing without external dependencies
//! - Single-process applications
//! - Development and prototyping

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use super::{Event, PublishError, Publisher, Subscribable, Subscriber};

/// Internal data for a named point-to-point queue.
/// Note: Point-to-point messaging is planned but not yet implemented.
#[derive(Default)]
#[allow(dead_code)]
struct PointToPointQueue {
    /// Messages in this queue
    messages: Vec<Event>,
    /// Shared position - all listeners compete for messages
    position: usize,
}

/// In-memory queue for testing and single-process scenarios.
///
/// Features:
/// - Thread-safe (can be shared across threads via `Clone`)
/// - Supports multiple subscribers via `new_subscriber()`
/// - Events are stored in an append-only log
/// - Each subscriber tracks its own read position
///
/// ## Example
///
/// ```
/// use sourced_rust::bus::{InMemoryQueue, Publisher, Subscriber, Event};
///
/// let queue = InMemoryQueue::new();
///
/// // Publish an event
/// queue.publish(Event::with_string_payload("evt-1", "OrderCreated", r#"{"id":"123"}"#)).unwrap();
///
/// // Poll for events
/// let event = queue.poll(100).unwrap();
/// assert!(event.is_some());
/// assert_eq!(event.unwrap().event_type, "OrderCreated");
/// ```
///
/// ## Multiple Subscribers
///
/// ```
/// use sourced_rust::bus::{InMemoryQueue, Publisher, Subscriber, Subscribable, Event};
///
/// let queue = InMemoryQueue::new();
/// queue.publish(Event::with_string_payload("evt-1", "Event1", "{}")).unwrap();
///
/// // Create independent subscribers
/// let sub1 = queue.new_subscriber();
/// let sub2 = queue.new_subscriber();
///
/// // Each subscriber has its own position
/// assert_eq!(sub1.poll(10).unwrap().unwrap().event_type, "Event1");
/// assert_eq!(sub2.poll(10).unwrap().unwrap().event_type, "Event1");
/// ```
#[derive(Clone)]
pub struct InMemoryQueue {
    /// Shared event log (for pub/sub - fan-out)
    log: Arc<RwLock<Vec<Event>>>,
    /// Per-subscriber read position (for pub/sub)
    position: Arc<Mutex<usize>>,
    /// Acknowledged event IDs
    acked: Arc<Mutex<Vec<String>>>,
    /// Named queues for point-to-point messaging (send/listen)
    queues: Arc<RwLock<HashMap<String, PointToPointQueue>>>,
}

impl Default for InMemoryQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryQueue {
    /// Create a new in-memory queue.
    pub fn new() -> Self {
        Self {
            log: Arc::new(RwLock::new(Vec::new())),
            position: Arc::new(Mutex::new(0)),
            acked: Arc::new(Mutex::new(Vec::new())),
            queues: Arc::new(RwLock::new(HashMap::new())),
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

    /// Get acknowledged event IDs.
    pub fn acknowledged(&self) -> Vec<String> {
        self.acked.lock().unwrap().clone()
    }

    /// Clear all events from the log (useful for test cleanup).
    pub fn clear(&self) {
        self.log.write().unwrap().clear();
        *self.position.lock().unwrap() = 0;
        self.acked.lock().unwrap().clear();
    }

    /// Subscribe to specific event types, returning a filtered receiver.
    ///
    /// The returned `EventReceiver` will only deliver events matching the
    /// specified types. Other events are skipped (but still consumed from
    /// this subscriber's position).
    ///
    /// ## Example
    ///
    /// ```
    /// use sourced_rust::bus::{InMemoryQueue, Publisher, Event};
    ///
    /// let queue = InMemoryQueue::new();
    /// queue.publish(Event::with_string_payload("evt-1", "OrderCreated", "{}")).unwrap();
    /// queue.publish(Event::with_string_payload("evt-2", "PaymentFailed", "{}")).unwrap();
    /// queue.publish(Event::with_string_payload("evt-3", "OrderCreated", "{}")).unwrap();
    ///
    /// // Subscribe only to OrderCreated events
    /// let receiver = queue.subscribe(&["OrderCreated"]);
    ///
    /// // Only receives OrderCreated events
    /// let event1 = receiver.recv(100).unwrap().unwrap();
    /// assert_eq!(event1.event_type, "OrderCreated");
    /// assert_eq!(event1.id, "evt-1");
    ///
    /// let event2 = receiver.recv(100).unwrap().unwrap();
    /// assert_eq!(event2.event_type, "OrderCreated");
    /// assert_eq!(event2.id, "evt-3");
    /// ```
    pub fn subscribe(&self, event_types: &[&str]) -> EventReceiver<Self> {
        EventReceiver::new(self.new_subscriber(), event_types)
    }
}

/// A filtered event receiver that only delivers events of subscribed types.
///
/// Created via [`InMemoryQueue::subscribe`] or [`Bus::subscribe`]. Each receiver
/// has its own position in the event log and only returns events matching the
/// subscribed types.
pub struct EventReceiver<S: Subscriber> {
    subscriber: S,
    event_types: HashSet<String>,
}

impl<S: Subscriber> EventReceiver<S> {
    /// Create a new event receiver with the given subscriber and event type filter.
    pub fn new(subscriber: S, event_types: &[&str]) -> Self {
        Self {
            subscriber,
            event_types: event_types.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// Receive the next matching event, blocking until one is available or timeout.
    ///
    /// Returns `Ok(Some(event))` if a matching event was found,
    /// `Ok(None)` if the timeout was reached with no matching events.
    pub fn recv(&self, timeout_ms: u64) -> Result<Option<Event>, PublishError> {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);

        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(None);
            }

            match self.subscriber.poll(remaining.as_millis() as u64)? {
                Some(event) if self.event_types.contains(&event.event_type) => {
                    return Ok(Some(event));
                }
                Some(_) => {
                    // Skip non-matching event, continue polling
                    continue;
                }
                None => {
                    return Ok(None);
                }
            }
        }
    }

    /// Try to receive an event without blocking.
    ///
    /// Returns immediately with `Ok(None)` if no matching event is available.
    pub fn try_recv(&self) -> Result<Option<Event>, PublishError> {
        self.recv(0)
    }

    /// Acknowledge that an event has been processed.
    pub fn ack(&self, event_id: &str) -> Result<(), PublishError> {
        self.subscriber.ack(event_id)
    }

    /// Get the event types this receiver is subscribed to.
    pub fn subscribed_types(&self) -> &HashSet<String> {
        &self.event_types
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
        // In-memory queue doesn't support redelivery; events stay in log
        Ok(())
    }
}

impl Subscribable for InMemoryQueue {
    fn new_subscriber(&self) -> Self {
        Self {
            log: Arc::clone(&self.log),
            position: Arc::new(Mutex::new(0)),
            acked: Arc::new(Mutex::new(Vec::new())),
            queues: Arc::clone(&self.queues),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publish_and_poll() {
        let queue = InMemoryQueue::new();

        queue
            .publish(Event::with_string_payload(
                "evt-1",
                "TestEvent",
                r#"{"data": 1}"#,
            ))
            .unwrap();

        let event = queue.poll(100).unwrap();
        assert!(event.is_some());
        let event = event.unwrap();
        assert_eq!(event.event_type, "TestEvent");
        assert_eq!(event.payload_str(), Some(r#"{"data": 1}"#));
    }

    #[test]
    fn poll_timeout_when_empty() {
        let queue = InMemoryQueue::new();
        let event = queue.poll(10).unwrap();
        assert!(event.is_none());
    }

    #[test]
    fn multiple_subscribers() {
        let queue = InMemoryQueue::new();

        queue
            .publish(Event::with_string_payload("evt-1", "Event1", "{}"))
            .unwrap();
        queue
            .publish(Event::with_string_payload("evt-2", "Event2", "{}"))
            .unwrap();

        // Create a second subscriber
        let sub2 = queue.new_subscriber();

        // First subscriber reads both events
        assert_eq!(queue.poll(10).unwrap().unwrap().event_type, "Event1");
        assert_eq!(queue.poll(10).unwrap().unwrap().event_type, "Event2");

        // Second subscriber also reads both events (independent position)
        assert_eq!(sub2.poll(10).unwrap().unwrap().event_type, "Event1");
        assert_eq!(sub2.poll(10).unwrap().unwrap().event_type, "Event2");
    }

    #[test]
    fn publish_batch() {
        let queue = InMemoryQueue::new();

        let events = vec![
            Event::with_string_payload("evt-1", "Event1", "{}"),
            Event::with_string_payload("evt-2", "Event2", "{}"),
            Event::with_string_payload("evt-3", "Event3", "{}"),
        ];

        queue.publish_batch(events).unwrap();

        assert_eq!(queue.len(), 3);
        assert_eq!(queue.event_types(), vec!["Event1", "Event2", "Event3"]);
    }

    #[test]
    fn find_by_type() {
        let queue = InMemoryQueue::new();

        queue
            .publish(Event::with_string_payload(
                "evt-1",
                "OrderCreated",
                r#"{"id":"1"}"#,
            ))
            .unwrap();
        queue
            .publish(Event::with_string_payload(
                "evt-2",
                "PaymentSucceeded",
                r#"{"id":"2"}"#,
            ))
            .unwrap();
        queue
            .publish(Event::with_string_payload(
                "evt-3",
                "OrderCreated",
                r#"{"id":"3"}"#,
            ))
            .unwrap();

        let found = queue.find_by_type("PaymentSucceeded");
        assert!(found.is_some());
        assert_eq!(found.unwrap().id, "evt-2");

        let all_orders = queue.find_all_by_type("OrderCreated");
        assert_eq!(all_orders.len(), 2);
    }

    #[test]
    fn clear_resets_state() {
        let queue = InMemoryQueue::new();

        queue
            .publish(Event::with_string_payload("evt-1", "Event1", "{}"))
            .unwrap();
        queue.poll(10).unwrap();
        queue.ack("evt-1").unwrap();

        assert_eq!(queue.len(), 1);
        assert_eq!(queue.current_position(), 1);
        assert_eq!(queue.acknowledged().len(), 1);

        queue.clear();

        assert_eq!(queue.len(), 0);
        assert_eq!(queue.current_position(), 0);
        assert!(queue.acknowledged().is_empty());
    }

    #[test]
    fn subscribe_filters_events() {
        let queue = InMemoryQueue::new();

        queue
            .publish(Event::with_string_payload("evt-1", "OrderCreated", "{}"))
            .unwrap();
        queue
            .publish(Event::with_string_payload("evt-2", "PaymentFailed", "{}"))
            .unwrap();
        queue
            .publish(Event::with_string_payload("evt-3", "InventoryReserved", "{}"))
            .unwrap();
        queue
            .publish(Event::with_string_payload("evt-4", "OrderCreated", "{}"))
            .unwrap();

        // Subscribe only to OrderCreated
        let receiver = queue.subscribe(&["OrderCreated"]);

        // Should only get OrderCreated events
        let event1 = receiver.recv(100).unwrap().unwrap();
        assert_eq!(event1.id, "evt-1");
        assert_eq!(event1.event_type, "OrderCreated");

        let event2 = receiver.recv(100).unwrap().unwrap();
        assert_eq!(event2.id, "evt-4");
        assert_eq!(event2.event_type, "OrderCreated");

        // No more matching events
        assert!(receiver.recv(10).unwrap().is_none());
    }

    #[test]
    fn subscribe_multiple_types() {
        let queue = InMemoryQueue::new();

        queue
            .publish(Event::with_string_payload("evt-1", "OrderCreated", "{}"))
            .unwrap();
        queue
            .publish(Event::with_string_payload("evt-2", "PaymentFailed", "{}"))
            .unwrap();
        queue
            .publish(Event::with_string_payload("evt-3", "OrderCompleted", "{}"))
            .unwrap();

        // Subscribe to multiple event types
        let receiver = queue.subscribe(&["OrderCreated", "OrderCompleted"]);

        let event1 = receiver.recv(100).unwrap().unwrap();
        assert_eq!(event1.id, "evt-1");

        let event2 = receiver.recv(100).unwrap().unwrap();
        assert_eq!(event2.id, "evt-3");

        assert!(receiver.recv(10).unwrap().is_none());
    }

    #[test]
    fn multiple_subscribers_independent() {
        let queue = InMemoryQueue::new();

        queue
            .publish(Event::with_string_payload("evt-1", "OrderCreated", "{}"))
            .unwrap();
        queue
            .publish(Event::with_string_payload("evt-2", "PaymentSucceeded", "{}"))
            .unwrap();

        // Two subscribers with different filters
        let orders = queue.subscribe(&["OrderCreated"]);
        let payments = queue.subscribe(&["PaymentSucceeded"]);

        // Each gets their own events
        assert_eq!(orders.recv(100).unwrap().unwrap().id, "evt-1");
        assert_eq!(payments.recv(100).unwrap().unwrap().id, "evt-2");
    }
}
