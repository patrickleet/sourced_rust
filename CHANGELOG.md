### What's changed in v1.7.1

* perf: make Kafka ack/nack non-blocking on the async runtime (#78) (by @patrickleet)

  Per-message offset commits used CommitMode::Sync, which blocks the calling
  OS thread on a broker round trip; called from the async `ack` (and
  dead_letter/park) for every message, combined with the strictly sequential
  consume loop in bus/runner.rs, this capped throughput and stalled the tokio
  worker. nack additionally called a blocking seek with a 5s timeout in async
  context.

  Switch offset commits to CommitMode::Async: the offset is handed to
  librdkafka's background thread and returns immediately. This is at-least-once
  (unchanged from the current contract): the runner already acks only after
  handler effects complete, so a crash before an async commit lands simply
  redelivers the record — a duplicate the consumer already tolerates.

  seek has no async variant, so run it on tokio's blocking pool via
  spawn_blocking (the Arc-shared consumer clones cleanly into the task).

  Implements [[tasks/kafka-nonblocking-ack]]


See full diff: [v1.7.0...v1.7.1](https://github.com/hops-ops/distributed/compare/v1.7.0...v1.7.1)
