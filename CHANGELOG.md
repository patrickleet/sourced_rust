### What's changed in v4.2.0

* feat: add cancellable OutboxDrainRunner (#203) (by @patrickleet)

  Immediate after-commit publish stays the fast path. This loop is the
  safety net: dispatch_batch until the outbox is empty, sleep when idle,
  back off on store errors, stop on abort.

  Worker id is drain:<pid> so claims never collide with
  microsvc-immediate:<pid>. Service::outbox_drain builds the runner;
  spawn_outbox_publish_loop now wraps it. Dropping the handle leaves the
  task running; stop() aborts.

  Implements [[tasks/outbox-background-drain-runner]]


See full diff: [v4.1.1...v4.2.0](https://github.com/hops-ops/distributed/compare/v4.1.1...v4.2.0)
