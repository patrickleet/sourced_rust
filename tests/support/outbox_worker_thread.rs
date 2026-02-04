//! Re-export OutboxWorkerThread from the library for tests.

pub use sourced_rust::{OutboxWorkerThread, WorkerStats};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::support::in_memory_queue::InMemoryQueue;
    use sourced_rust::bus::Subscriber;
    use sourced_rust::{Commit, HashMapRepository, OutboxMessage};
    use std::thread;
    use std::time::Duration;

    #[test]
    fn worker_publishes_outbox_messages() {
        // HashMapRepository uses Arc<RwLock<...>> internally, so cloning
        // creates another handle to the same storage
        let repo = HashMapRepository::new();
        let queue = InMemoryQueue::new();

        // Start the worker with a clone of the repo
        let worker = OutboxWorkerThread::spawn(
            repo.clone(),
            queue.clone(),
            Duration::from_millis(10),
        );

        // Create an outbox message directly in the repo
        {
            let mut outbox = OutboxMessage::create(
                "test-msg-1",
                "TestEvent",
                br#"{"data": "hello"}"#.to_vec(),
            );
            repo.commit(&mut outbox.entity).unwrap();
        }

        // Wait for the worker to process
        thread::sleep(Duration::from_millis(100));

        // Stop the worker and get stats
        let stats = worker.stop();

        // Verify the message was published
        assert!(stats.messages_published >= 1);
        assert_eq!(queue.len(), 1);

        let event = queue.poll(10).unwrap().unwrap();
        assert_eq!(event.event_type, "TestEvent");
    }
}
