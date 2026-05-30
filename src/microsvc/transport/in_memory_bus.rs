//! In-memory bus — the dev/test reference implementation of [`Bus`] +
//! [`BusConsumer`].
//!
//! `send`/`listen` use named queues with competing-consumer (point-to-point)
//! semantics: a message is popped by exactly one consumer. `publish`/`subscribe`
//! use named **retained logs** with a per-subscriber cursor, so every subscriber
//! sees every event (fan-out) — the same log+offset shape the Postgres fan-out
//! transport uses, in memory.
//!
//! It is intentionally simple (no durability, no redelivery on nack) — for tests
//! and local development. Use a real transport for production reliability.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use super::source::{AsyncMessageSource, ReceivedMessage};
use super::{run_source, Bus, BusConsumer, RunOptions, TransportError};
use crate::microsvc::{Message, MessageKind, Service};

type Queues = Arc<Mutex<HashMap<String, VecDeque<Message>>>>;
type Topics = Arc<Mutex<HashMap<String, Vec<Message>>>>;

fn lock_poisoned(what: &str) -> TransportError {
    TransportError::permanent(format!("in-memory bus {what} lock poisoned"))
}

/// In-memory [`Bus`] + [`BusConsumer`] for tests and local development.
///
/// Cheap to clone (shares the same queues/logs), so competing listeners and
/// fan-out subscribers can each hold a clone.
#[derive(Clone, Default)]
pub struct InMemoryBus {
    queues: Queues,
    topics: Topics,
}

impl InMemoryBus {
    pub fn new() -> Self {
        Self::default()
    }

    fn enqueue(&self, message: Message) -> Result<(), TransportError> {
        self.queues
            .lock()
            .map_err(|_| lock_poisoned("queue"))?
            .entry(message.name().to_string())
            .or_default()
            .push_back(message);
        Ok(())
    }

    fn append(&self, message: Message) -> Result<(), TransportError> {
        self.topics
            .lock()
            .map_err(|_| lock_poisoned("topic"))?
            .entry(message.name().to_string())
            .or_default()
            .push(message);
        Ok(())
    }
}

impl Bus for InMemoryBus {
    async fn send(&self, name: &str, payload: Vec<u8>) -> Result<(), TransportError> {
        self.enqueue(Message::new(name, MessageKind::Command, payload))
    }

    async fn publish(&self, name: &str, payload: Vec<u8>) -> Result<(), TransportError> {
        self.append(Message::new(name, MessageKind::Event, payload))
    }

    async fn send_message(&self, message: Message) -> Result<(), TransportError> {
        self.enqueue(message)
    }

    async fn publish_message(&self, message: Message) -> Result<(), TransportError> {
        self.append(message)
    }
}

impl BusConsumer for InMemoryBus {
    async fn listen<D: Send + Sync + 'static>(
        &self,
        service: Arc<Service<D>>,
        options: RunOptions,
    ) -> Result<(), TransportError> {
        let names = service
            .command_names()
            .iter()
            .map(|n| n.to_string())
            .collect();
        let source = QueueSource {
            queues: self.queues.clone(),
            names,
        };
        run_source(service, source, options).await
    }

    async fn subscribe<D: Send + Sync + 'static>(
        &self,
        service: Arc<Service<D>>,
        options: RunOptions,
    ) -> Result<(), TransportError> {
        let names = service
            .event_names()
            .iter()
            .map(|n| n.to_string())
            .collect();
        let source = TopicSource {
            topics: self.topics.clone(),
            names,
            cursors: HashMap::new(),
        };
        run_source(service, source, options).await
    }
}

/// Competing-consumer source over the named queues: each message is popped once.
struct QueueSource {
    queues: Queues,
    names: Vec<String>,
}

impl AsyncMessageSource for QueueSource {
    type Received = InMemoryReceived;

    async fn recv(&mut self) -> Result<Option<Self::Received>, TransportError> {
        let mut queues = self.queues.lock().map_err(|_| lock_poisoned("queue"))?;
        for name in &self.names {
            if let Some(message) = queues.get_mut(name).and_then(VecDeque::pop_front) {
                return Ok(Some(InMemoryReceived { message }));
            }
        }
        Ok(None)
    }
}

/// Fan-out source over the named retained logs: each `TopicSource` has its own
/// cursor, so every subscriber reads every event.
struct TopicSource {
    topics: Topics,
    names: Vec<String>,
    cursors: HashMap<String, usize>,
}

impl AsyncMessageSource for TopicSource {
    type Received = InMemoryReceived;

    async fn recv(&mut self) -> Result<Option<Self::Received>, TransportError> {
        let topics = self.topics.lock().map_err(|_| lock_poisoned("topic"))?;
        for name in &self.names {
            let Some(log) = topics.get(name) else {
                continue;
            };
            let cursor = self.cursors.entry(name.clone()).or_insert(0);
            if *cursor < log.len() {
                let message = log[*cursor].clone();
                *cursor += 1;
                return Ok(Some(InMemoryReceived { message }));
            }
        }
        Ok(None)
    }
}

/// In-memory delivery. Settling is a no-op: queue pops and log cursors already
/// advanced on `recv`, and the in-memory bus does not redeliver.
pub struct InMemoryReceived {
    message: Message,
}

impl ReceivedMessage for InMemoryReceived {
    fn message(&self) -> &Message {
        &self.message
    }
    async fn ack(self) -> Result<(), TransportError> {
        Ok(())
    }
    async fn nack(self, _reason: &str) -> Result<(), TransportError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::microsvc::HandlerError;
    use serde_json::json;
    use std::future::Future;

    fn block_on<F: Future>(future: F) -> F::Output {
        use std::ptr;
        use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
        const VTABLE: RawWakerVTable = RawWakerVTable::new(
            |_| RawWaker::new(ptr::null(), &VTABLE),
            |_| {},
            |_| {},
            |_| {},
        );
        let waker = unsafe { Waker::from_raw(RawWaker::new(ptr::null(), &VTABLE)) };
        let mut cx = Context::from_waker(&waker);
        let mut future = std::pin::pin!(future);
        loop {
            if let Poll::Ready(output) = future.as_mut().poll(&mut cx) {
                return output;
            }
        }
    }

    fn recorder() -> Arc<Mutex<Vec<String>>> {
        Arc::new(Mutex::new(Vec::new()))
    }

    fn command_service(rec: Arc<Mutex<Vec<String>>>) -> Arc<Service<()>> {
        Arc::new(Service::new(()).command("work").handle(
            move |ctx: &crate::microsvc::Context<()>| {
                let rec = rec.clone();
                let name = ctx.message().name().to_string();
                async move {
                    rec.lock().unwrap().push(name);
                    Ok(json!({}))
                }
            },
        ))
    }

    fn event_service(rec: Arc<Mutex<Vec<String>>>) -> Arc<Service<()>> {
        Arc::new(
            Service::new(())
                .event("evt")
                .handle(move |ctx: &crate::microsvc::Context<()>| {
                    let rec = rec.clone();
                    let id = ctx.message().id().unwrap_or("?").to_string();
                    async move {
                        rec.lock().unwrap().push(id);
                        Ok(json!({}))
                    }
                }),
        )
    }

    #[test]
    fn send_then_listen_dispatches_each_command() {
        let bus = InMemoryBus::new();
        for _ in 0..3 {
            block_on(bus.send("work", b"{}".to_vec())).unwrap();
        }
        let rec = recorder();
        block_on(bus.listen(command_service(rec.clone()), RunOptions::idempotent())).unwrap();
        assert_eq!(
            rec.lock().unwrap().len(),
            3,
            "the listener handles all 3 commands"
        );
    }

    #[test]
    fn listen_is_point_to_point_each_message_popped_once() {
        // Two competing sources over the same queue: each message goes to one.
        let bus = InMemoryBus::new();
        for i in 0..4 {
            block_on(bus.send_message(
                Message::new("work", MessageKind::Command, b"{}".to_vec()).with_id(format!("m{i}")),
            ))
            .unwrap();
        }
        let mut a = QueueSource {
            queues: bus.queues.clone(),
            names: vec!["work".to_string()],
        };
        let mut b = QueueSource {
            queues: bus.queues.clone(),
            names: vec!["work".to_string()],
        };
        let mut got = Vec::new();
        // Alternate; each pop removes the message (competing).
        for _ in 0..4 {
            if let Some(r) = block_on(a.recv()).unwrap() {
                got.push(r.message().id().unwrap().to_string());
            }
            if let Some(r) = block_on(b.recv()).unwrap() {
                got.push(r.message().id().unwrap().to_string());
            }
        }
        got.sort();
        assert_eq!(
            got,
            vec!["m0", "m1", "m2", "m3"],
            "each message delivered exactly once"
        );
        // Queue now drained for both.
        assert!(block_on(a.recv()).unwrap().is_none());
        assert!(block_on(b.recv()).unwrap().is_none());
    }

    #[test]
    fn publish_then_subscribe_fans_out_to_every_subscriber() {
        let bus = InMemoryBus::new();
        for i in 0..3 {
            block_on(bus.publish_message(
                Message::new("evt", MessageKind::Event, b"{}".to_vec()).with_id(format!("e{i}")),
            ))
            .unwrap();
        }
        // Two independent subscribers; each gets every event (own cursor).
        let a = recorder();
        let b = recorder();
        block_on(bus.subscribe(event_service(a.clone()), RunOptions::idempotent())).unwrap();
        block_on(bus.subscribe(event_service(b.clone()), RunOptions::idempotent())).unwrap();
        let mut a_ids = a.lock().unwrap().clone();
        let mut b_ids = b.lock().unwrap().clone();
        a_ids.sort();
        b_ids.sort();
        assert_eq!(a_ids, vec!["e0", "e1", "e2"]);
        assert_eq!(b_ids, vec!["e0", "e1", "e2"]);
    }

    #[test]
    fn unknown_command_is_acked_and_ignored() {
        // A command with no handler is ignored by the runner (acked), not an error.
        let bus = InMemoryBus::new();
        block_on(bus.send("unrelated", b"{}".to_vec())).unwrap();
        block_on(bus.send("work", b"{}".to_vec())).unwrap();
        let rec = recorder();
        block_on(bus.listen(command_service(rec.clone()), RunOptions::idempotent())).unwrap();
        assert_eq!(rec.lock().unwrap().clone(), vec!["work"]);
    }

    #[test]
    fn handler_error_does_not_panic_the_loop() {
        let bus = InMemoryBus::new();
        block_on(bus.send("work", b"{}".to_vec())).unwrap();
        let service: Arc<Service<()>> =
            Arc::new(Service::new(()).command("work").handle(
                |_: &crate::microsvc::Context<()>| async move {
                    Err(HandlerError::Rejected("no".into()))
                },
            ));
        // Default failure policy dead-letters the permanent failure; in-memory
        // dead_letter is a no-op nack, so the run completes cleanly.
        block_on(bus.listen(service, RunOptions::idempotent())).unwrap();
    }
}
