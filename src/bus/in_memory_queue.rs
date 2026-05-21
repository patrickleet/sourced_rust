//! In-memory queue for testing and single-process scenarios.
//!
//! This module provides a thread-safe in-memory queue that implements
//! both `Publisher` and `Subscriber` traits, useful for:
//! - Unit and integration testing without external dependencies
//! - Single-process applications
//! - Development and prototyping

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::{Duration, Instant};

use super::{Event, Listener, PublishError, Publisher, Sender, Subscribable, Subscriber};

/// Internal data for a named point-to-point queue.
#[derive(Default)]
struct PointToPointQueue {
    /// Messages in this queue
    messages: Vec<Event>,
    /// Shared position - all listeners compete for messages
    position: usize,
}

fn lock_poisoned(lock_name: &str) -> PublishError {
    PublishError::ConnectionFailed(format!("in-memory queue {lock_name} lock poisoned"))
}

fn lock_mutex<'a, T>(
    mutex: &'a Mutex<T>,
    lock_name: &str,
) -> Result<MutexGuard<'a, T>, PublishError> {
    mutex.lock().map_err(|_| lock_poisoned(lock_name))
}

fn read_lock<'a, T>(
    lock: &'a RwLock<T>,
    lock_name: &str,
) -> Result<RwLockReadGuard<'a, T>, PublishError> {
    lock.read().map_err(|_| lock_poisoned(lock_name))
}

fn write_lock<'a, T>(
    lock: &'a RwLock<T>,
    lock_name: &str,
) -> Result<RwLockWriteGuard<'a, T>, PublishError> {
    lock.write().map_err(|_| lock_poisoned(lock_name))
}

fn recover_mutex<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn recover_read<T>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    match lock.read() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn recover_write<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    match lock.write() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
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
/// use sourced_rust::bus::{Event, InMemoryQueue, PublishError, Publisher, Subscriber};
///
/// fn main() -> Result<(), PublishError> {
///     let queue = InMemoryQueue::new();
///
///     queue.publish(Event::with_string_payload("evt-1", "OrderCreated", r#"{"id":"123"}"#))?;
///
///     let event = queue.poll(100)?.ok_or(PublishError::Timeout)?;
///     assert_eq!(event.event_type, "OrderCreated");
///     Ok(())
/// }
/// ```
///
/// ## Multiple Subscribers
///
/// ```
/// use sourced_rust::bus::{
///     Event, InMemoryQueue, PublishError, Publisher, Subscribable, Subscriber,
/// };
///
/// fn main() -> Result<(), PublishError> {
///     let queue = InMemoryQueue::new();
///     queue.publish(Event::with_string_payload("evt-1", "Event1", "{}"))?;
///
///     let sub1 = queue.new_subscriber();
///     let sub2 = queue.new_subscriber();
///
///     assert_eq!(sub1.poll(10)?.ok_or(PublishError::Timeout)?.event_type, "Event1");
///     assert_eq!(sub2.poll(10)?.ok_or(PublishError::Timeout)?.event_type, "Event1");
///     Ok(())
/// }
/// ```
///
/// ## Point-to-Point Queues
///
/// ```
/// use sourced_rust::bus::{Event, InMemoryQueue, Listener, PublishError, Sender};
///
/// fn main() -> Result<(), PublishError> {
///     let queue = InMemoryQueue::new();
///     queue.send("orders", Event::with_string_payload("evt-1", "ProcessOrder", "{}"))?;
///
///     let event = queue.listen("orders", 100)?.ok_or(PublishError::Timeout)?;
///     assert_eq!(event.event_type, "ProcessOrder");
///     Ok(())
/// }
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
        recover_read(&self.log).clone()
    }

    /// Get all event types in order.
    pub fn event_types(&self) -> Vec<String> {
        recover_read(&self.log)
            .iter()
            .map(|e| e.event_type.clone())
            .collect()
    }

    /// Get the total number of events in the log.
    pub fn len(&self) -> usize {
        recover_read(&self.log).len()
    }

    /// Check if the log is empty.
    pub fn is_empty(&self) -> bool {
        recover_read(&self.log).is_empty()
    }

    /// Find an event by type.
    pub fn find_by_type(&self, event_type: &str) -> Option<Event> {
        recover_read(&self.log)
            .iter()
            .find(|e| e.event_type == event_type)
            .cloned()
    }

    /// Find all events matching a type.
    pub fn find_all_by_type(&self, event_type: &str) -> Vec<Event> {
        recover_read(&self.log)
            .iter()
            .filter(|e| e.event_type == event_type)
            .cloned()
            .collect()
    }

    /// Reset the subscriber position to the beginning.
    pub fn reset_position(&self) {
        *recover_mutex(&self.position) = 0;
    }

    /// Get the current subscriber position.
    pub fn current_position(&self) -> usize {
        *recover_mutex(&self.position)
    }

    /// Get acknowledged event IDs.
    pub fn acknowledged(&self) -> Vec<String> {
        recover_mutex(&self.acked).clone()
    }

    /// Clear all events from the log (useful for test cleanup).
    pub fn clear(&self) {
        self.log.clear_poison();
        self.position.clear_poison();
        self.acked.clear_poison();
        self.queues.clear_poison();

        recover_write(&self.log).clear();
        *recover_mutex(&self.position) = 0;
        recover_mutex(&self.acked).clear();
        recover_write(&self.queues).clear();
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
    /// use sourced_rust::bus::{Event, InMemoryQueue, PublishError, Publisher};
    ///
    /// fn main() -> Result<(), PublishError> {
    ///     let queue = InMemoryQueue::new();
    ///     queue.publish(Event::with_string_payload("evt-1", "OrderCreated", "{}"))?;
    ///     queue.publish(Event::with_string_payload("evt-2", "PaymentFailed", "{}"))?;
    ///     queue.publish(Event::with_string_payload("evt-3", "OrderCreated", "{}"))?;
    ///
    ///     let receiver = queue.subscribe(&["OrderCreated"]);
    ///
    ///     let event1 = receiver.recv(100)?.ok_or(PublishError::Timeout)?;
    ///     assert_eq!(event1.event_type, "OrderCreated");
    ///     assert_eq!(event1.id, "evt-1");
    ///
    ///     let event2 = receiver.recv(100)?.ok_or(PublishError::Timeout)?;
    ///     assert_eq!(event2.event_type, "OrderCreated");
    ///     assert_eq!(event2.id, "evt-3");
    ///     Ok(())
    /// }
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
        write_lock(&self.log, "event log")?.push(event);
        Ok(())
    }

    fn publish_batch(&self, events: Vec<Event>) -> Result<(), PublishError> {
        let mut log = write_lock(&self.log, "event log")?;
        log.extend(events);
        Ok(())
    }
}

impl Subscriber for InMemoryQueue {
    fn poll(&self, timeout_ms: u64) -> Result<Option<Event>, PublishError> {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);

        loop {
            {
                let log = read_lock(&self.log, "event log")?;
                let mut pos = lock_mutex(&self.position, "subscriber position")?;

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
        lock_mutex(&self.acked, "acknowledgement list")?.push(event_id.to_string());
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

impl Sender for InMemoryQueue {
    fn send(&self, queue: &str, event: Event) -> Result<(), PublishError> {
        let mut queues = write_lock(&self.queues, "point-to-point queues")?;
        queues
            .entry(queue.to_string())
            .or_default()
            .messages
            .push(event);
        Ok(())
    }
}

impl Listener for InMemoryQueue {
    fn listen(&self, queue: &str, timeout_ms: u64) -> Result<Option<Event>, PublishError> {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);

        loop {
            {
                let mut queues = write_lock(&self.queues, "point-to-point queues")?;
                if let Some(q) = queues.get_mut(queue) {
                    if q.position < q.messages.len() {
                        let event = q.messages[q.position].clone();
                        q.position += 1;
                        return Ok(Some(event));
                    }
                }
            }

            if Instant::now() >= deadline {
                return Ok(None);
            }

            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn poison_lock<F>(poison: F)
    where
        F: FnOnce(),
    {
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(poison)).is_err());
    }

    fn assert_lock_poisoned<T: std::fmt::Debug>(result: Result<T, PublishError>, lock_name: &str) {
        let err = result.expect_err("poisoned lock should return an error");
        match err {
            PublishError::ConnectionFailed(msg) => {
                assert!(msg.contains(lock_name), "unexpected lock error: {msg}");
                assert!(
                    msg.contains("lock poisoned"),
                    "unexpected lock error: {msg}"
                );
            }
            other => panic!("unexpected error for poisoned lock: {other:?}"),
        }
    }

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
    fn publish_returns_error_when_log_lock_is_poisoned() {
        let queue = InMemoryQueue::new();
        poison_lock(|| {
            let _guard = queue.log.write().expect("lock event log");
            panic!("poison event log");
        });

        assert_lock_poisoned(
            queue.publish(Event::with_string_payload("evt-1", "Event1", "{}")),
            "event log",
        );
    }

    #[test]
    fn poll_returns_error_when_position_lock_is_poisoned() {
        let queue = InMemoryQueue::new();
        queue
            .publish(Event::with_string_payload("evt-1", "Event1", "{}"))
            .unwrap();
        poison_lock(|| {
            let _guard = queue.position.lock().expect("lock subscriber position");
            panic!("poison subscriber position");
        });

        assert_lock_poisoned(queue.poll(0), "subscriber position");
    }

    #[test]
    fn ack_returns_error_when_ack_lock_is_poisoned() {
        let queue = InMemoryQueue::new();
        poison_lock(|| {
            let _guard = queue.acked.lock().expect("lock acknowledgement list");
            panic!("poison acknowledgement list");
        });

        assert_lock_poisoned(queue.ack("evt-1"), "acknowledgement list");
    }

    #[test]
    fn send_returns_error_when_queue_lock_is_poisoned() {
        let queue = InMemoryQueue::new();
        poison_lock(|| {
            let _guard = queue.queues.write().expect("lock point-to-point queues");
            panic!("poison point-to-point queues");
        });

        assert_lock_poisoned(
            queue.send("tasks", Event::with_string_payload("evt-1", "Task", "{}")),
            "point-to-point queues",
        );
    }

    #[test]
    fn accessors_recover_from_poisoned_locks() {
        let queue = InMemoryQueue::new();
        poison_lock(|| {
            let _guard = queue.log.write().expect("lock event log");
            panic!("poison event log");
        });
        assert!(queue.events().is_empty());

        let queue = InMemoryQueue::new();
        poison_lock(|| {
            let _guard = queue.position.lock().expect("lock subscriber position");
            panic!("poison subscriber position");
        });
        assert_eq!(queue.current_position(), 0);

        let queue = InMemoryQueue::new();
        poison_lock(|| {
            let _guard = queue.acked.lock().expect("lock acknowledgement list");
            panic!("poison acknowledgement list");
        });
        assert!(queue.acknowledged().is_empty());
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
        queue
            .send("tasks", Event::with_string_payload("task-1", "Task", "{}"))
            .unwrap();

        assert_eq!(queue.len(), 1);
        assert_eq!(queue.current_position(), 1);
        assert_eq!(queue.acknowledged().len(), 1);
        assert!(queue.listen("tasks", 0).unwrap().is_some());
        queue
            .send("tasks", Event::with_string_payload("task-2", "Task", "{}"))
            .unwrap();

        queue.clear();

        assert_eq!(queue.len(), 0);
        assert_eq!(queue.current_position(), 0);
        assert!(queue.acknowledged().is_empty());
        assert!(queue.listen("tasks", 0).unwrap().is_none());
    }

    #[test]
    fn clear_resets_lock_poisoning_for_normal_operations() {
        let queue = InMemoryQueue::new();
        queue
            .publish(Event::with_string_payload("evt-1", "Event1", "{}"))
            .unwrap();
        queue.poll(10).unwrap();
        queue.ack("evt-1").unwrap();

        poison_lock(|| {
            let _guard = queue.log.write().expect("lock event log");
            panic!("poison event log");
        });
        poison_lock(|| {
            let _guard = queue.position.lock().expect("lock subscriber position");
            panic!("poison subscriber position");
        });
        poison_lock(|| {
            let _guard = queue.acked.lock().expect("lock acknowledgement list");
            panic!("poison acknowledgement list");
        });
        poison_lock(|| {
            let _guard = queue.queues.write().expect("lock point-to-point queues");
            panic!("poison point-to-point queues");
        });

        queue.clear();

        queue
            .publish(Event::with_string_payload("evt-2", "Event2", "{}"))
            .unwrap();
        assert_eq!(queue.poll(10).unwrap().unwrap().id, "evt-2");
        queue.ack("evt-2").unwrap();
        queue
            .send("tasks", Event::with_string_payload("task-1", "Task", "{}"))
            .unwrap();
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
            .publish(Event::with_string_payload(
                "evt-3",
                "InventoryReserved",
                "{}",
            ))
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
    fn send_and_listen() {
        let queue = InMemoryQueue::new();

        queue
            .send(
                "orders",
                Event::with_string_payload("evt-1", "ProcessOrder", r#"{"id":"123"}"#),
            )
            .unwrap();

        let event = queue.listen("orders", 100).unwrap();
        assert!(event.is_some());
        let event = event.unwrap();
        assert_eq!(event.event_type, "ProcessOrder");
        assert_eq!(event.id, "evt-1");
    }

    #[test]
    fn listen_timeout_when_empty() {
        let queue = InMemoryQueue::new();
        let event = queue.listen("orders", 10).unwrap();
        assert!(event.is_none());
    }

    #[test]
    fn send_listen_competing_consumers() {
        let queue = InMemoryQueue::new();

        queue
            .send("tasks", Event::with_string_payload("evt-1", "Task", "{}"))
            .unwrap();
        queue
            .send("tasks", Event::with_string_payload("evt-2", "Task", "{}"))
            .unwrap();

        // Two clones share the same queues (competing consumers)
        let consumer1 = queue.clone();
        let consumer2 = queue.clone();

        // Each consumer gets a different message
        let e1 = consumer1.listen("tasks", 100).unwrap().unwrap();
        let e2 = consumer2.listen("tasks", 100).unwrap().unwrap();
        assert_eq!(e1.id, "evt-1");
        assert_eq!(e2.id, "evt-2");

        // No more messages
        assert!(queue.listen("tasks", 10).unwrap().is_none());
    }

    #[test]
    fn send_listen_separate_queues() {
        let queue = InMemoryQueue::new();

        queue
            .send("orders", Event::with_string_payload("evt-1", "Order", "{}"))
            .unwrap();
        queue
            .send(
                "payments",
                Event::with_string_payload("evt-2", "Payment", "{}"),
            )
            .unwrap();

        // Each queue is independent
        let order = queue.listen("orders", 100).unwrap().unwrap();
        assert_eq!(order.id, "evt-1");

        let payment = queue.listen("payments", 100).unwrap().unwrap();
        assert_eq!(payment.id, "evt-2");

        // Queues don't cross-contaminate
        assert!(queue.listen("orders", 10).unwrap().is_none());
        assert!(queue.listen("payments", 10).unwrap().is_none());
    }

    #[test]
    fn multiple_subscribers_independent() {
        let queue = InMemoryQueue::new();

        queue
            .publish(Event::with_string_payload("evt-1", "OrderCreated", "{}"))
            .unwrap();
        queue
            .publish(Event::with_string_payload(
                "evt-2",
                "PaymentSucceeded",
                "{}",
            ))
            .unwrap();

        // Two subscribers with different filters
        let orders = queue.subscribe(&["OrderCreated"]);
        let payments = queue.subscribe(&["PaymentSucceeded"]);

        // Each gets their own events
        assert_eq!(orders.recv(100).unwrap().unwrap().id, "evt-1");
        assert_eq!(payments.recv(100).unwrap().unwrap().id, "evt-2");
    }
}
