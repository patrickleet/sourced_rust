### What's changed in v1.5.0

* feat: add distributed project manifest primitives (by @patrickleet)

* feat: return CommitReceipt from outbox commit (by @patrickleet)

  OutboxCommit::commit now returns a CommitReceipt carrying the inserted
  outbox message id(s) instead of (), so an after-commit dispatcher can
  publish exactly the rows the transaction wrote. Source-compatible:
  ?-statement callers discard the receipt.

  Step 1 of [[tasks/durable-enqueue-outbox-dispatch-impl]]
  Implements [[specs/durable-enqueue-outbox-dispatch]]

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

* feat: add BusPublisher (Bus -> AsyncMessagePublisher) adapter (by @patrickleet)

  Routes outbox-derived messages by MessageKind: commands to send_message
  (point-to-point), events to publish_message (fan-out). This is the missing
  adapter that lets the outbox dispatcher publish through any *Bus uniformly.

  Step 2 of [[tasks/durable-enqueue-outbox-dispatch-impl]]
  Implements [[specs/durable-enqueue-outbox-dispatch]]

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

* feat: add HasOutboxStore capability for repo wrappers (by @patrickleet)

  New trait abstracting 'produce a durable outbox store', resolving through the
  AggregateRepository -> QueuedRepository -> leaf repo wrapper chain. Lets the
  runtime build an OutboxDispatcher without naming the concrete repository type.
  Impls for HashMap (and feature-gated Sqlite/Postgres) leaves + the wrappers.

  Step 3 (store access) of [[tasks/durable-enqueue-outbox-dispatch-impl]]
  Implements [[specs/durable-enqueue-outbox-dispatch]]

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

* feat: add Service::with_bus + Microservice runtime (produce side) (by @patrickleet)

  Service::with_bus(bus) wraps the consumer Service into a Microservice carrying
  the transport config. Microservice::dispatcher() assembles an OutboxDispatcher
  over the service's own outbox store + a BusPublisher, so committed outbox rows
  drain to the bus routed by kind. Test proves commit -> dispatch -> published
  end to end over InMemoryBus.

  Consume side (run() auto listen/subscribe) and the in-transaction commit_outbox
  land next.

  Step 6 (runtime, produce side) of [[tasks/durable-enqueue-outbox-dispatch-impl]]
  Implements [[specs/durable-enqueue-outbox-dispatch]]

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

* feat: add OutboxCommit::commit_claimed (claim-in-transaction) (by @patrickleet)

  Claims the outbox row for publication in the same transaction that commits the
  aggregate: the row inserts already InFlight under the worker's lease
  (attempts = 1), so the after-commit publish needs no separate claim and cannot
  race the poller. Returns the claimed message clone so the caller can build the
  transport message and settle the claim. Test proves the row is in-flight,
  leased, and not poller-claimable.

  Step 4 (claim-in-transaction) of [[tasks/durable-enqueue-outbox-dispatch-impl]]
  Implements [[specs/durable-enqueue-outbox-dispatch]]

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

* feat: Context::commit_outbox — publish-in-commit via attached bus (by @patrickleet)

  Wires the durable-enqueue command path end to end:
  - DynPublisher: object-safe (boxed-future) form of AsyncMessagePublisher, so a
    publisher can sit behind Arc<dyn> without making Service generic over it.
  - Service carries an optional ImmediatePublish (publisher + worker id + lease +
    attempts), set by with_bus; Context receives it.
  - Context::commit_outbox: with a bus attached, claims the outbox row in the
    commit transaction then publishes immediately through the bus, completing or
    releasing the claim; with no bus, commits pending for the poller. Best-effort
    publish never rolls back the committed aggregate.

  Test: dispatch -> commit_outbox -> row published immediately, none left pending.

  Steps 3+5 (DynPublisher + commit_outbox) of [[tasks/durable-enqueue-outbox-dispatch-impl]]
  Implements [[specs/durable-enqueue-outbox-dispatch]]

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

* feat: Microservice::run derives listen/subscribe from handlers (by @patrickleet)

  run() reads the service's subscription_plan and drives the consumers
  concurrently on the caller's runtime: command handlers via competing listen,
  event handlers via fan-out subscribe. Uses an executor-agnostic poll-join (no
  spawn, no timer) so it works in core without pulling tokio. Returns on first
  error or when the consumers stop. Derive Clone for RunOptions/ConsumerDeliveryMode
  so one options value drives both consumers. Test: run() consumes a queued
  command and the handler's commit_outbox publishes immediately.

  Producing happy-path is commit_outbox (immediate); the backstop poll loop (needs
  a timer) is driven from dispatcher() by a runtime that provides one.

  Step 6 (runtime, consume side) of [[tasks/durable-enqueue-outbox-dispatch-impl]]
  Implements [[specs/durable-enqueue-outbox-dispatch]]

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

* test: SQLite end-to-end durable-enqueue dispatch (by @patrickleet)

  Exercises commit_outbox (claim-in-transaction + immediate publish) and run()
  against a real SQL backend (in-memory SQLite), not just HashMapRepository.
  Proves the HasOutboxStore impls and the SQL commit path persist the in-flight
  claim and complete it. Also fixes a must_use warning on the finished-consumer
  future in run().

  [[tasks/durable-enqueue-outbox-dispatch-impl]]
  Implements [[specs/durable-enqueue-outbox-dispatch]]

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

* feat: commit_outbox works for all repo shapes incl snapshots (by @patrickleet)

  Generalize the durable-enqueue command path with an OutboxCommitting<A> trait
  that commits an aggregate + outbox row in one transaction, staging whatever the
  repo needs. Implemented for AggregateRepository (delegates to the existing
  OutboxCommit) and SnapshotAggregateRepository (stages the snapshot + outbox row
  together via CommitBatch — previously these could not compose). Context::commit_outbox
  now binds D::Repo: OutboxCommitting<A> + HasOutboxStore instead of the concrete
  AggregateRepository, so snapshot-backed services get claim-in-transaction +
  immediate publish too. Test: snapshot-backed commit_outbox publishes immediately.

  [[tasks/durable-enqueue-outbox-dispatch-impl]]
  Builds on [[specs/transactional-commit-boundary]]
  Implements [[specs/durable-enqueue-outbox-dispatch]]

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

* refactor: snapshots are a transparent optimization (one repo type) (by @patrickleet)

  Fold SnapshotAggregateRepository into AggregateRepository via an optional
  SnapshotPolicy whose Snapshottable/SnapshotStore requirements are captured as
  monomorphized fn-pointers at with_snapshots() time, keeping the generic
  get/commit methods unbounded. Now:

  - .with_snapshots(n) returns AggregateRepository<R,A> (same type), so handler
    dependency types are identical with/without snapshots.
  - every method works either way; commit stages a snapshot (when due) in the same
    CommitBatch, get hydrates from a snapshot when present. The full repo surface
    (peek/abort/get_with/outbox/...) is available with snapshots on — previously
    the wrapper dropped most of it.
  - exactly ONE OutboxCommitting impl (on AggregateRepository); the snapshot-
    specific impl and the whole SnapshotAggregateRepository type are removed.

  with_snapshots now requires R: SnapshotStore (you can't cache snapshots in a
  store that can't hold them) — stricter and more correct than the old wrapper.
  Tests migrated to the unified type; assertions unchanged. Full suite + sqlite green.

  Implements [[specs/snapshots-as-transparent-optimization]] [[tasks/snapshots-transparent-optimization]]

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

* refactor: repo.outbox(msg).commit(agg) publishes — no wrapper (by @patrickleet)

  Make the existing API do the new functionality instead of adding a method.
  Attaching a bus (Service::with_bus) installs an outbox publisher on the
  repository; OutboxCommit::commit then claims the row in the commit transaction
  and publishes it immediately via that bus, settling the claim (complete, or
  release for the worker on failure). No bus configured -> commit stays pending
  for the worker, exactly as before.

  Removed: ctx.commit_outbox, the OutboxCommitting trait, OutboxCommit::commit_claimed,
  Service's ImmediatePublish + the Context publisher plumbing, and the now-unused
  DynPublisher (the snapshot unification already collapsed the two repo types, so
  the polymorphism trait was dead weight).

  Added: OutboxPublishHook (object-safe) + OutboxPublisherConfig on the repo,
  BusOutboxPublishHook (store + BusPublisher), ConfigurableOutboxPublisher. Tests
  migrated to repo.outbox(msg).commit(agg); full default + sqlite suites green.

  Implements [[specs/durable-enqueue-outbox-dispatch]]

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

* refactor: with_bus is a Service builder step, not a separate type (by @patrickleet)

  Fold Microservice back into Service. Attaching a bus no longer changes the
  type: with_bus(bus) returns the same Service<D> and run() is a method on it, so
  the whole thing reads as one fluent builder —
    Service::with_repo(r).command(..).handle(..).with_bus(bus).run(opts)

  The bus's consume behavior is type-erased into a single closure field on the
  service (ServiceRunner), so Service stays single-param — message_router, the
  register_handlers! macro, and every existing Service<D> call site are untouched.
  Removes the Microservice type and the speculative dispatcher() accessor (the
  backstop poll loop is a later, runtime-gated addition).

  Net simpler: one type, one builder, less code.

  Implements [[specs/durable-enqueue-outbox-dispatch]]

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

* refactor: Service::new().with_repo().with_read_model_store() builder (by @patrickleet)

  Replace the with_repo / with_read_model_store / with_repo_and_read_model_store
  constructors with one fluent builder: every service starts at Service::new() and
  chains dependency + bus steps —
    Service::new().with_repo(r).with_read_model_store(s).with_bus(bus)

  with_repo/with_read_model_store are type-state transitions that produce exactly
  the same D as before (Service<R>, or RepoReadModelDependencies<R,S> for both), so
  handler signatures are unchanged — only construction call sites move. Combined
  deps now delegate HasOutboxStore + ConfigurableOutboxPublisher to the repo so a
  repo+read-model service can also with_bus. Migrated all call sites + README.

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

* docs: README durable-enqueue framing + close the Quick Start produce loop (by @patrickleet)

  Update the Quick Start to the Service::new().with_repo(..).with_bus(bus).run()
  builder and make the produce loop explicit: step 2 commits an outbox row, step 3
  attaches a bus so that commit publishes on commit. Rewrite Draining the Outbox
  as Publishing the Outbox (immediate-on-commit vs pending+worker), and document
  the backstop poll loop as the composable OutboxDispatcher + your timer. Note the
  with_bus().run() convenience alongside the lower-level listen/subscribe facade.

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

* feat: compose read-models + snapshots in one aggregate commit (by @patrickleet)

  Close the last 'all features compose' gap. The AggregateRepository-level commit
  (formerly OutboxCommit, now AggregateCommit) carries outbox rows AND read-model
  write plans, and stages a snapshot from the repo's policy — all in one
  CommitBatch. New entry repo.read_models(plan) mirrors repo.outbox(msg); both
  chain (.outbox(..).read_models(..)) and end in .commit(agg), which also publishes
  the outbox rows on commit when a bus is attached.

  Previously read-model commits ran at the raw-repo level (CommitBuilder, no
  snapshot policy) so snapshots and read-models could not compose. Now a
  snapshot-backed repo commits streams + outbox + read-models + snapshot
  atomically. Test proves read-model row + snapshot land in one transaction.

  The raw-repo CommitBuilder (repo: &R) is unchanged for non-aggregate-repo use.

  Implements [[specs/durable-enqueue-outbox-dispatch]] [[tasks/snapshot-readmodel-commit-compose]]

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

* test: aggregate + outbox + read-model + snapshot in one transaction (by @patrickleet)

  Single test exercising all four staged in one commit
  (repo.outbox(msg).read_models(plan).commit(agg) on a with_snapshots(1) repo):
  asserts the aggregate stream, outbox row, read-model row, and snapshot all
  land together.

  [[tasks/snapshot-readmodel-commit-compose]]

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

* feat: distributed_tooling crate — pure service scaffold generation (by @patrickleet)

  New workspace crate implementing specs/distributed-service-scaffold-tooling: a
  pure ServiceScaffoldSpec -> GeneratedProject API (no fs, network, or CLI). Owns
  the deterministic generation rules — name/message normalization + validation,
  GitHub repo parsing, and the core service-crate templates (Cargo.toml, lib/main/
  manifest/service/models/handlers/read_models). Returns GeneratedFile list +
  warnings + PostCreateAction (EnsureGithubRepository) for the caller to act on.

  Generated service.rs uses the new Service::new().with_repo(repo) builder. The
  public API includes the gitops/github spec fields; those artifact templates +
  the hops-cli rewire are the next slices.

  7 tests green; workspace builds; clippy clean.

  Implements [[specs/distributed-service-scaffold-tooling]] [[tasks/distributed-tooling-crate-extraction]]

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

* refactor: split distributed_tooling generate.rs into focused modules (by @patrickleet)

  generate/ becomes a module: mod.rs (Scaffold + orchestration + entry + tests),
  names.rs (name/message normalization + validation), service_crate.rs (the Rust
  templates as impl Scaffold), github.rs (repo parsing). Sets up gitops.rs and the
  GitHub workflow templates as their own files for the next slice. No behavior
  change; 7 tests green.

  Implements [[specs/distributed-service-scaffold-tooling]]

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

* feat: port GitOps + GitHub workflow generation into distributed_tooling (by @patrickleet)

  generate/gitops.rs: .gitops/deploy Helm chart (HTTP Deployment+Service or
  Knative Service+Brokers+Triggers) + optional .gitops/promote Argo/Flux chart,
  with Knative broker/trigger inference and image-repo selection. generate/github.rs
  gains the release/preview/promote workflow templates + the Argo CD promotion
  chart. generate() now emits these (deploy chart whenever any gitops/github option
  is set, matching the original); the placeholder warning is gone.

  10 crate tests (added GitOps HTTP, Knative brokers/triggers, Flux promote, and
  full GitHub workflow coverage). Next: the hops-cli rewire.

  Implements [[specs/distributed-service-scaffold-tooling]] [[tasks/distributed-tooling-crate-extraction]]

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

* refactor: model the three GitHub scaffold flags independently (by @patrickleet)

  The original hops-cli scaffold exposes --github, --github-preview, and
  --github-promote as independent flags: --github emits the version/release
  workflows and the repo-create action; --github-preview emits the preview
  workflow + .gitops/preview/helm chart; --github-promote emits the promote
  workflow + .gitops/promote/helm chart. Each can be set without the others
  (e.g. preview-only), and only --github triggers repo creation.

  The crate previously nested preview/promote under a required GithubScaffoldSpec
  repository, which could not represent preview-only and tied the workflows to a
  service repo. Replace it with three flat Option<GithubRepo> fields on
  ServiceScaffoldSpec (github / github_preview / github_promote), mirroring the
  flags 1:1 and dropping the GithubScaffoldSpec wrapper. The deploy chart is now
  emitted when any of the five gitops/github signals is set. Adds a regression
  test for the preview-only path.

  Prepares the faithful hops-cli rewire onto this crate.

  Implements [[specs/distributed-service-scaffold-tooling]] [[tasks/distributed-tooling-crate-extraction]]

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

* feat: expose package_name() for default output-dir derivation (by @patrickleet)

  The CLI adapter needs the normalized kebab package name to compute the default
  output directory (./<name>) before generating. Expose the existing
  ScaffoldNames normalization as a public helper instead of duplicating the
  casing rule in the CLI.

  Implements [[tasks/distributed-tooling-crate-extraction]]

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

* fix: address PR #53 review — generated-output correctness + builder guards (by @patrickleet)

  - reject service/model names that yield invalid Rust identifiers, instead of
    emitting a crate that won't compile
  - deploy Helm templates honor image.repository/tag rather than hardcoding
    :latest, so values.yaml/release automation actually drives the image
  - dedupe Knative trigger names that normalize to the same metadata.name, which
    otherwise breaks `kubectl apply`
  - fail fast when dependency builders (with_repo/with_read_model_store) run after
    handler/bus setup, which silently dropped registrations
  - reject snapshot frequency 0 (would snapshot on every commit)
  - correct outbox claim-timing wording: the row is claimed post-commit under a
    short lease, not within the commit transaction

  Implements [[tasks/distributed-tooling-crate-extraction]]

  Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>

* ci: publish distributed_tooling on version tags (by @patrickleet)

  distributed_tooling was a workspace member but absent from the release
  pipeline, so it could never become a crates.io dependency. Add a
  publish-tooling job alongside publish-macros (the crate only depends on
  serde_json, so it has no internal publish ordering) and gate the release on it.

  Implements [[tasks/distributed-tooling-crate-extraction]]

  Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>


See full diff: [v1.4.1...v1.5.0](https://github.com/hops-ops/distributed/compare/v1.4.1...v1.5.0)
