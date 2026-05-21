### What's changed in v0.1.0

* : 

* : 

* : 

* : 

* : 

* feat: mutable find by id (#4)

* fix: cleanup

* feat: commands -> events

* chore: update readme

* chore: update readme

* chore: update readme

* chore: update readme

* fix: refactor

* fix: refactor

* fix: refactor

* fix: refactor

* fix: refactor

* fix: refactor

* fix: refactor

* fix: refactor

* feat: vscode debugger

* feat: rust workflow

* fix: unused Clone impl

* fix: refactor find_by_id -> get

* feat: get_all and commit_all

* feat: tests

* feat: Repository trait + HashMapRepository (#5)

  * feat: tests

  * feat: refactor example

  * feat: refactory repo into trait and hashmap implementation of it

  * fix: refactor tests

* fix: more tests

* feat: refactor + outbox pattern support (#6)

  * feat: 1

  Signed-off-by: Patrick Lee Scott <pat@patscott.io>

  * fix: agent overcomplicated things based on preconceived notions of event sourcing instead of understand the code... undo

  Signed-off-by: Patrick Lee Scott <pat@patscott.io>

  * feat: commit with nothing feels wrong - added abort to release lock

  Signed-off-by: Patrick Lee Scott <pat@patscott.io>

  * feat: peek + readme

  Signed-off-by: Patrick Lee Scott <pat@patscott.io>

  * feat: outbox + refactor

  Signed-off-by: Patrick Lee Scott <pat@patscott.io>

  * feat: aggregate repository

  Signed-off-by: Patrick Lee Scott <pat@patscott.io>

  * feat: aggregate refactor

  Signed-off-by: Patrick Lee Scott <pat@patscott.io>

  * feat: sugar

  Signed-off-by: Patrick Lee Scott <pat@patscott.io>

  * fix: abort without commit tests

  Signed-off-by: Patrick Lee Scott <pat@patscott.io>

  ---------

  Signed-off-by: Patrick Lee Scott <pat@patscott.io>

* chore: update readme

  Signed-off-by: Patrick Lee Scott <pat@patscott.io>

* feat: boundaries + outbox refactor (#7)

  Signed-off-by: Patrick Lee Scott <pat@patscott.io>

* feat: traits + macro + more complicated tests (#8)

* fix: macro cleanup

* chore: Add link to original sourced Node.js project

* chore: Fix acronym in README description

  Corrected the acronym from PORO to PORS in the description.

* feat: saga test

* feat: disributed test + outbox worker + in memory queue for testing without external deps

* feat: refactor

* fix: saga thread

* feat: bus.subscribe

* chore: update readme

* feat: poc

* feat: refactor

* feat: refactor

* feat: refactor

* feat: refactor

* feat: readmodel

* feat: queued read model

* feat: commit_many entities

* chore: refactor bus files (by @patrickleet)

* feat: send/listen + refactor (by @patrickleet)

* feat: domain services - take 1 (by @patrickleet)

* feat: distributed/stateless locking interface (by @patrickleet)

* chore: readme (by @patrickleet)

* feat: snapshots (by @patrickleet)

* chore: cargo (by @patrickleet)

* feat: enqueue tests + fixes (by @patrickleet)

* chore: readme (by @patrickleet)

* feat: metadata for correlation / causation id and etc (by @patrickleet)

* feat: event upcasters (by @patrickleet)

* chore: readme (by @patrickleet)

* feat: sourced macro + event enums (by @patrickleet)

* feat: enqueue option in sourced macro (by @patrickleet)

* feat: microsvc command handlers (by @patrickleet)

* feat: microsvc tests (by @patrickleet)

* feat: queued repo implements clone (by @patrickleet)

* fix: cleanup (by @patrickleet)

* chore: microservice saga test (by @patrickleet)

* feat: service + command handler tests version of saga (by @patrickleet)

* chore: rm poc domain service - microsvc does this idea better (by @patrickleet)

* feat: snapshot macro (by @patrickleet)

* feat: domain_event message helper (by @patrickleet)

* feat: transport cleanup + fixes (by @patrickleet)

* feat: microsvc grpc transport (by @patrickleet)

* chore: grpc readme update (by @patrickleet)

* fix: cleanup (by @patrickleet)

* feat: Make mixed commits transactional (#13) (by @patrickleet)

  * feat: make mixed commits transactional

  * chore: git kb init

  * chore: ignore tmp workdir

  * style: standardize rustfmt output

* fix: harden event record compatibility (by @patrickleet)

  Implements [[tasks/harden-event-record-compatibility]]

* refactor: make event recording API fallible (by @patrickleet)

* refactor: infer sourced result for event macros (by @patrickleet)

* refactor: make upcast_events fallible (by @patrickleet)

* refactor: propagate handler event errors (by @patrickleet)

* fix: return commit builder serialization errors (by @patrickleet)

* fix: return outbox lease deadline errors (by @patrickleet)

* fix: return enqueue serialization errors (by @patrickleet)

* fix: handle inferred macro tail expressions (by @patrickleet)

  Implements [[tasks/review-macro-tail-result-wrapping]]

* fix: reset snapshot committed version (by @patrickleet)

  Implements [[tasks/review-set-snapshot-committed-version]]

* fix: default legacy event payload codec (by @patrickleet)

  Implements [[tasks/review-event-record-codec-defaults]]

* fix: reject backward upcaster transitions (by @patrickleet)

  Implements [[tasks/review-upcaster-backward-transition]]

* fix: count only newly claimed outbox messages (by @patrickleet)

  Implements [[tasks/review-outbox-claimed-count]]

* fix: validate in-memory find one rows (by @patrickleet)

  Implements [[tasks/review-in-memory-find-one-validation]]

* fix: record power up collection only on removal (by @patrickleet)

  Implements [[tasks/review-power-up-digest-condition]]

* fix: return bomberman outbox serialization errors (by @patrickleet)

  Implements [[tasks/review-bomberman-outbox-serialization]]

* docs: remove duplicate postgres stream index (by @patrickleet)

  Implements [[tasks/review-postgres-duplicate-index-doc]]

* refactor: model power up collection as digest command (by @patrickleet)

  Refs [[tasks/review-power-up-digest-condition]]

* chore: skills config (by @patrickleet)

* fix: return gRPC address parse errors (#17) (by @patrickleet)

  Implements [[tasks/remove-grpc-address-parse-panic]]

* fix: return outbox worker join errors (#18) (by @patrickleet)

  Implements [[tasks/return-outbox-worker-join-errors]]

* fix: return microsvc transport join errors (#19) (by @patrickleet)

  Implements [[tasks/return-microsvc-transport-join-errors]]

* fix: clarify queued repo lock lifecycle (#22) (by @patrickleet)

  Implements [[tasks/queued-repo-lock-lifecycle]]

* chore(docs): propagate errors in queue examples (#23) (by @patrickleet)

  Implements [[tasks/remove-doc-example-unwraps]]

* chore(docs): clarify event and CQRS terminology (#26) (by @patrickleet)

  Implements [[tasks/event-sourcing-cqrs-terminology]]

* fix: guard read model version increments (#25) (by @patrickleet)

  Implements [[tasks/audit-version-default-fallbacks]]

* fix: harden outbox claim leases (#28) (by @patrickleet)

  * fix: harden outbox claim leases

  Implements [[tasks/durable-outbox-claim-leases]]

  * fix: address outbox worker review feedback

* chore: deploy key secret (by @patrickleet)


