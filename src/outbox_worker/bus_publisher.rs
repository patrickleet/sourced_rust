//! `Bus` → `MessagePublisher` adapter.
//!
//! The outbox dispatcher publishes through a single [`MessagePublisher`],
//! but the command-vs-event topology split lives in the [`Bus`] as two methods
//! (`send_message` for point-to-point commands, `publish_message` for fan-out
//! events) backed by different publishers/topologies. This adapter bridges them
//! by routing on [`MessageKind`] — which the outbox → [`Message`] mapping
//! already sets (`Command` when a `destination` is present, else `Event`).
//!
//! This is what lets the outbox dispatcher publish through any `*Bus` uniformly,
//! for both the after-commit immediate path and the background poll loop.

use std::sync::Arc;

use crate::bus::{Bus, Message, MessageKind, MessagePublisher, TransportError};

/// Publishes outbox-derived [`Message`]s through a [`Bus`], routing by kind:
/// commands to `send_message` (point-to-point), events to `publish_message`
/// (fan-out).
pub struct BusPublisher<B> {
    bus: Arc<B>,
}

impl<B> BusPublisher<B> {
    /// Wrap a shared bus as a [`MessagePublisher`].
    pub fn new(bus: Arc<B>) -> Self {
        Self { bus }
    }

    /// The wrapped bus.
    pub fn bus(&self) -> &Arc<B> {
        &self.bus
    }
}

impl<B> Clone for BusPublisher<B> {
    fn clone(&self) -> Self {
        Self {
            bus: Arc::clone(&self.bus),
        }
    }
}

impl<B: Bus> MessagePublisher for BusPublisher<B> {
    async fn publish(&self, message: Message) -> Result<(), TransportError> {
        match message.kind {
            MessageKind::Command => self.bus.send_message(message).await,
            MessageKind::Event => self.bus.publish_message(message).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outbox_worker::testing::block_on;
    use std::sync::Mutex;

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Call {
        Send(String),
        Publish(String),
    }

    /// A bus that records which produce method was used for each message.
    #[derive(Default)]
    struct RecordingBus {
        calls: Mutex<Vec<Call>>,
    }

    impl RecordingBus {
        fn calls(&self) -> Vec<Call> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl Bus for RecordingBus {
        async fn send(&self, name: &str, payload: Vec<u8>) -> Result<(), TransportError> {
            self.send_message(Message::new(name, MessageKind::Command, payload))
                .await
        }
        async fn publish(&self, name: &str, payload: Vec<u8>) -> Result<(), TransportError> {
            self.publish_message(Message::new(name, MessageKind::Event, payload))
                .await
        }
        async fn send_message(&self, message: Message) -> Result<(), TransportError> {
            self.calls.lock().unwrap().push(Call::Send(message.name));
            Ok(())
        }
        async fn publish_message(&self, message: Message) -> Result<(), TransportError> {
            self.calls.lock().unwrap().push(Call::Publish(message.name));
            Ok(())
        }
    }

    #[test]
    fn routes_command_to_send_and_event_to_publish() {
        let bus = Arc::new(RecordingBus::default());
        let publisher = BusPublisher::new(bus.clone());

        // A command-kind message (outbox row with a destination) → send_message.
        block_on(publisher.publish(Message::new(
            "ship.order",
            MessageKind::Command,
            b"{}".to_vec(),
        )))
        .unwrap();
        // An event-kind message → publish_message.
        block_on(publisher.publish(Message::new(
            "order.shipped",
            MessageKind::Event,
            b"{}".to_vec(),
        )))
        .unwrap();

        assert_eq!(
            bus.calls(),
            vec![
                Call::Send("ship.order".to_string()),
                Call::Publish("order.shipped".to_string()),
            ]
        );
    }
}
