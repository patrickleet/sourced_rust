### What's changed in v0.3.0

* feat: unify read-model ORM write plans (by @patrickleet)

  Implements [[specs/read-model-orm-unification]].

  Covers [[tasks/read-model-orm-01-inventory]], [[tasks/read-model-orm-02-metadata]], [[tasks/read-model-orm-03-session-write-plan]], [[tasks/read-model-orm-04-commit-builder-bridge]], [[tasks/read-model-orm-05-compat-conformance]], [[tasks/read-model-orm-06-distributed-idempotency]], [[tasks/read-model-orm-07-schema-bootstrap]], and [[tasks/read-model-orm-08-test-migration-docs]].

* fix: harden read-model derive metadata parsing (by @patrickleet)

* fix: avoid relational row key fingerprint collisions (by @patrickleet)

* fix: validate read-model relationship foreign keys (by @patrickleet)

* feat: add read-model helper attributes (by @patrickleet)

  Adds direct ReadModel helper attributes for collection, table, column, id, field indexes, unique indexes, and struct-level compound indexes. Updates distributed, metadata, session, schema, and docs coverage.

  Implements [[tasks/read-model-orm-09-compound-indexes]].

* test: organize distributed read model services (by @patrickleet)

  Moves the distributed read-model integration test into account_service, projections_service, and query_service modules while preserving existing behavior.

* feat: add tracked read model relationship includes (by @patrickleet)

* fix: address read model review feedback (by @patrickleet)

* fix: guard primary keys in row patches (by @patrickleet)

* test: assert failed sparse insert leaves no row (by @patrickleet)

* feat: delete removed has_many children on save_changes (by @patrickleet)

  Make `save_changes` reconcile included collections to the struct: an owned
  has_many child dropped from the loaded Vec is deleted, lowering to an explicit
  DeleteRow with the loaded expected version. belongs_to clear-to-None stays a
  no-op on the target. Safe because has_many includes load the complete owned set.

  Replaces the prior "removal does not delete by default" behavior, which was
  asymmetric (auto-persisted adds/edits but silently dropped removals).

  Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>

* test: distributed relational read-model examples with fulfillment saga (by @patrickleet)

  Rework tests/distributed_read_model into a Catalog + Order CQRS slice over
  normalized relational read models (ProductView, OrderView has_many
  OrderLineView belongs_to ProductView, JSONB columns), and add a kanban
  Board + Cards example. Add an order-fulfillment saga (inventory, payment,
  saga orchestrator) driving confirm/cancel with a compensation path, projected
  into an OrderFulfillmentStepView has_many child for a multi-include query.

  Conventions: each write service is a microsvc::Service with service.rs +
  handlers/ (one file per message) + models/ (aggregate); the projection service
  is one dispatcher organized into handler modules; published domain events are
  lowercase dot-namespaced. Services publish via the outbox and subscribe via
  microsvc::subscribe — no bespoke transport.

  Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>

* :  (by @patrickleet)

* :  (by @patrickleet)

* fix: store binary read-model rows as bytes (by @patrickleet)

* fix: reject duplicate read-model relationship attrs (by @patrickleet)

* fix: fail document plans on unsupported mutations (by @patrickleet)

* fix: reject duplicate read-model index names (by @patrickleet)

* fix: fingerprint document read-model keys (by @patrickleet)

* fix: release queued read-model locks once (by @patrickleet)

* fix: fail board projection on malformed event versions (by @patrickleet)

* fix: reject non-positive product creation prices (by @patrickleet)

* fix: validate distributed inventory quantities (by @patrickleet)

* fix: guard distributed order line edits (by @patrickleet)

* fix: fail order projection on malformed event versions (by @patrickleet)

* test: assert idempotency not-found variants (by @patrickleet)


See full diff: [v0.2.1...v0.3.0](https://github.com/patrickleet/sourced_rust/compare/v0.2.1...v0.3.0)
