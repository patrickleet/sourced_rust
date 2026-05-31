### What's changed in v1.2.0

* feat: extract the bus into a standalone `crate::bus` module (#51) (by @patrickleet)

  * refactor(bus): introduce MessageRouter consume seam (Phase 1)

  Decouple the consume path from the concrete microsvc::Service:

  - Add `MessageRouter` { handles, subscription_plan, dispatch } in
    transport/router.rs — the trait run_source and the BusConsumer adapters
    depend on instead of Service<D>.
  - impl MessageRouter for Service<D> (microsvc/message_router.rs); the
    HandlerError -> TransportError classification happens on the microsvc side
    so the runner only sees an already-classified error.
  - run_source<R: MessageRouter, S, I: Send>: drop the <D> Service generic,
    keep the <I> inbox-hook generic (RunOptions::inbox still works).
  - BusConsumer::listen/subscribe take Arc<impl MessageRouter>; rewrite all five
    adapters (in_memory/nats/rabbitmq/kafka/postgres) + rabbit::ensure_subscription
    to derive topology from subscription_plan() instead of command_names()/
    event_names().

  No behavior change. 226 lib + 414 integration + 12 in-memory conformance tests
  green; cargo check --all-features clean.

  Implements [[tasks/bus-decomposition-phase1]] / [[specs/bus-module-decomposition]]

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

  * feat(bus): add dependency-free Handlers inline registry (Phase 2)

  Handlers is the second MessageRouter impl and the standalone, Service-free
  way to consume the bus: register a closure per (kind, name) and run it with
  bus.listen/bus.subscribe — the Rust analog of Node's bus.listen('x', fn).

  - Reuses the AsyncMessageHandler HRTB boxed-future pattern (over &Message, no
    Context/deps/guards); idempotent-only by design.
  - A Service<()> facade is impossible (bus must not depend on microsvc), so
    Handlers is its own engine.
  - Tests prove full InMemoryBus publish->subscribe and send->listen round trips
    with no Service.

  Implements [[tasks/bus-decomposition-phase1]] / [[specs/bus-module-decomposition]]

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

  * refactor(bus): relocate Message/MessageKind into bus-core message module (Phase 3)

  Move the canonical transport vocabulary out of microsvc::service into a
  dependency-free transport::message module, so the bus stops returning a
  microsvc error type from payload decoding:

  - New transport/message.rs: Message, MessageKind, and a bus-core
    PayloadDecodeError. payload_json/payload_bitcode now return PayloadDecodeError
    instead of microsvc::HandlerError.
  - microsvc::error gains From<PayloadDecodeError> for HandlerError; Context::input
    maps back, so handler signatures are unchanged.
  - Re-export Message/MessageKind/PayloadDecodeError from both transport and
    microsvc, so every `crate::microsvc::Message` consumer is unaffected.

  No behavior change. 229 lib + full default integration suite + 12 conformance
  green; cargo check --all-features clean.

  Implements [[tasks/bus-decomposition-phase1]] / [[specs/bus-module-decomposition]]

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

  * refactor(bus)!: lift transport/ to top-level src/bus module (Phase 4a)

  The bus is no longer embedded under microsvc: `src/microsvc/transport/` is
  now `src/bus/` (crate::bus). microsvc keeps a transitional `pub use crate::bus
  as transport;` alias so existing `microsvc::transport::…` paths (incl. tests)
  keep resolving; call sites and the bus's own upward imports are cleaned in P4b.

  Implements [[tasks/bus-decomposition-phase1]] / [[specs/bus-module-decomposition]]

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

  * refactor(bus): move SubscriptionPlan into bus-core message module (P4b/1)

  SubscriptionPlan is bus vocabulary (consumed by MessageRouter + adapters);
  move it out of microsvc::service into bus::message. Re-exported from microsvc
  for source compatibility.

  Implements [[tasks/bus-decomposition-phase1]]

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

  * refactor(bus): move outbox->bus bridge into outbox_worker (P4b/2)

  OutboxDispatcher/OutboxSource depend on crate::outbox + crate::outbox_worker,
  so they belong with the worker (which depends up on the bus's publisher/source
  traits), not in bus core. Re-exported at crate root; the two test crates that
  used them via microsvc::transport now import from the crate root.

  Implements [[tasks/bus-decomposition-phase1]]

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

  * refactor(bus): flip bus files' Message/MessageKind imports to super (P4b/3)

  Mechanical: bus production files now reference the canonical types via
  super:: (crate::bus) instead of crate::microsvc::. Remaining crate::microsvc
  references in bus are error.rs (B4), knative ingress (B5), and Service-based
  test modules (B6).

  Implements [[tasks/bus-decomposition-phase1]]

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

  * refactor(bus): invert error coupling + split Knative ingress (P4b/4)

  - bus/error.rs no longer names HandlerError: the From<HandlerError> conversion
    and classification move to microsvc (HandlerError::transport_error_kind);
    From<RepositoryError> for TransportError moves to the outbox bridge (which
    knows both types). Bus core's error.rs is now microsvc-free.
  - Knative split: the Service-coupled HTTP ingress (cloud_events_router /
    ingress_handler / CloudEvent parsing) moves to microsvc::knative_ingress;
    bus/knative.rs keeps only the Message/SubscriptionPlan-only manifest helpers
    (knative_triggers, sanitize_k8s_name). Two test crates updated.

  229 lib tests green; cargo check --all-features clean.

  Implements [[tasks/bus-decomposition-phase1]]

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

  * refactor(bus): make bus tests Service-free; bus is now microsvc-free (P4b/5)

  - Rewrite the runner and in_memory_bus unit tests to register handlers via
    the dependency-free Handlers builder instead of microsvc::Service, so the
    bus has zero crate::microsvc references (production AND tests).
  - Fix the rabbitmq_transport test to pass &Service (ensure_subscription now
    takes &impl MessageRouter, which doesn't deref-coerce &Arc).
  - cargo fmt.

  src/bus/ now imports nothing from microsvc. 229 lib + 493 default integration
  tests green; cargo test --all-features --no-run clean.

  Implements [[tasks/bus-decomposition-phase1]] / [[specs/bus-module-decomposition]]

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

  * refactor(bus)!: drop the microsvc::transport alias — clean break to crate::bus

  Remove the transitional pub use crate::bus as transport; alias and move every
  remaining reference to crate::bus / sourced_rust::bus: the message_router and
  context internal imports, the outbox_source test, all 11 test crates, and the
  README + async-transports docs. microsvc::transport no longer exists anywhere.

  493 default integration tests green; cargo test --all-features --no-run clean.

  Implements [[tasks/bus-decomposition-phase1]] / [[specs/bus-module-decomposition]]

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

  * docs(bus): fix intra-doc links to relocated bus types

  Point the bus module-doc links at the types' new homes after the extraction:
  OutboxDispatcher/OutboxDispatchOutcome/OutboxSource -> crate root re-exports,
  cloud_events_router -> crate::microsvc. Brings cargo doc back to parity with
  main (4 pre-existing warnings, zero new). Found by the branch-wide review.

  Implements [[tasks/bus-decomposition-phase1]]

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

  * fix(bus): dedupe Knative Trigger names to avoid manifest collisions

  Distinct CloudEvent types can normalize to the same RFC-1123 label (e.g.
  `order.created` and `order-created`, or a command and an event of the same
  name), so the generated `Trigger` `metadata.name`s could collide and the second
  would clobber the first on apply — silently dropping a subscription.

  Add `unique_k8s_name`, which sanitizes and then de-duplicates against the names
  already emitted (numeric suffix, capped at 63 chars; the `type:` filter still
  carries the raw event name, so routing is unchanged). Thread one dedup set
  through both generators: `knative_triggers` and `KnativeBus::manifests`
  (commands + events share the set). Addresses CodeRabbit review on PR #51.

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

  * style: rustfmt import ordering in two test crates

  Leftover formatting from the clean-break import sweep (rustfmt sorts the bus
  import before the microsvc one). No behavior change.

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

  * docs(bus): describe router-based dispatch in the module overview

  After the MessageRouter seam landed, the module header still said run_source
  dispatches through `Service::dispatch_message`. Update it to reference the
  `MessageRouter` consume seam (implemented by microsvc::Service and the
  dependency-free Handlers builder) and `MessageRouter::dispatch`, so rustdoc
  matches the new public surface. Addresses CodeRabbit review on PR #51.

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

  ---------

  Co-authored-by: Claude Opus 4.8 (1M context) <noreply@anthropic.com>


See full diff: [v1.1.0...v1.2.0](https://github.com/patrickleet/sourced_rust/compare/v1.1.0...v1.2.0)
