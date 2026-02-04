//! Service Bus - wraps publisher and subscriber for a service.

use super::{Event, PublishError, Publisher, Subscriber};

/// Service bus - wraps publisher and subscriber for a service.
///
/// The Bus provides a unified interface for both publishing events and
/// subscribing to events. Each service in a distributed system would
/// have its own Bus instance.
///
/// ## Example
///
/// ```ignore
/// // Create a bus with separate publisher and subscriber
/// let bus = Bus::new(kafka_publisher, kafka_subscriber);
///
/// // Or from a unified queue implementation
/// let bus = Bus::from_queue(in_memory_queue);
///
/// // Publish events
/// bus.publish(Event::with_string_payload("evt-1", "OrderCreated", "{}"))?;
///
/// // Poll for events
/// if let Some(event) = bus.poll(1000)? {
///     // Process event
///     bus.ack(&event.id)?;
/// }
/// ```
pub struct Bus<P: Publisher, S: Subscriber> {
    publisher: P,
    subscriber: S,
}

impl<P: Publisher, S: Subscriber> Bus<P, S> {
    /// Create a new bus with the given publisher and subscriber.
    pub fn new(publisher: P, subscriber: S) -> Self {
        Self {
            publisher,
            subscriber,
        }
    }

    /// Publish an event to the bus.
    pub fn publish(&self, event: Event) -> Result<(), PublishError> {
        self.publisher.publish(event)
    }

    /// Publish multiple events to the bus.
    pub fn publish_batch(&self, events: Vec<Event>) -> Result<(), PublishError> {
        self.publisher.publish_batch(events)
    }

    /// Poll for the next event, blocking until one is available or timeout.
    pub fn poll(&self, timeout_ms: u64) -> Result<Option<Event>, PublishError> {
        self.subscriber.poll(timeout_ms)
    }

    /// Acknowledge that an event has been processed.
    pub fn ack(&self, event_id: &str) -> Result<(), PublishError> {
        self.subscriber.ack(event_id)
    }

    /// Reject an event (will be redelivered or sent to dead letter queue).
    pub fn nack(&self, event_id: &str, reason: &str) -> Result<(), PublishError> {
        self.subscriber.nack(event_id, reason)
    }

    /// Get a reference to the underlying publisher.
    pub fn publisher(&self) -> &P {
        &self.publisher
    }

    /// Get a reference to the underlying subscriber.
    pub fn subscriber(&self) -> &S {
        &self.subscriber
    }
}

// Convenience: when publisher and subscriber are the same type (e.g., InMemoryQueue)
impl<T: Publisher + Subscriber + Clone> Bus<T, T> {
    /// Create a bus from a unified queue that implements both Publisher and Subscriber.
    ///
    /// This is useful for queue implementations that handle both directions,
    /// like InMemoryQueue or some message broker clients.
    pub fn from_queue(queue: T) -> Self {
        Self {
            publisher: queue.clone(),
            subscriber: queue,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    // Mock publisher for testing
    struct MockPublisher {
        events: Arc<Mutex<Vec<Event>>>,
    }

    impl Publisher for MockPublisher {
        fn publish(&self, event: Event) -> Result<(), PublishError> {
            self.events.lock().unwrap().push(event);
            Ok(())
        }
    }

    // Mock subscriber for testing
    struct MockSubscriber {
        events: Arc<Mutex<Vec<Event>>>,
        position: Arc<Mutex<usize>>,
    }

    impl Subscriber for MockSubscriber {
        fn poll(&self, _timeout_ms: u64) -> Result<Option<Event>, PublishError> {
            let events = self.events.lock().unwrap();
            let mut pos = self.position.lock().unwrap();
            if *pos < events.len() {
                let event = events[*pos].clone();
                *pos += 1;
                Ok(Some(event))
            } else {
                Ok(None)
            }
        }

        fn ack(&self, _event_id: &str) -> Result<(), PublishError> {
            Ok(())
        }

        fn nack(&self, _event_id: &str, _reason: &str) -> Result<(), PublishError> {
            Ok(())
        }
    }

    #[test]
    fn bus_publish_and_poll() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let publisher = MockPublisher {
            events: Arc::clone(&events),
        };
        let subscriber = MockSubscriber {
            events: Arc::clone(&events),
            position: Arc::new(Mutex::new(0)),
        };

        let bus = Bus::new(publisher, subscriber);

        bus.publish(Event::with_string_payload("evt-1", "TestEvent", "{}"))
            .unwrap();

        let event = bus.poll(100).unwrap();
        assert!(event.is_some());
        assert_eq!(event.unwrap().event_type, "TestEvent");
    }
}
