### What's changed in v0.6.0

* feat: persist read models as relational rows (by @patrickleet)

  Remove the generic document read-model store path and lower read-model write plans into declared relational tables for in-memory, SQLite, and Postgres repositories.

  Update docs, migrations, tests, and the Bomberman example to use relational read-model schemas and handlers.

  Verified with cargo fmt --all, cargo test --test bomberman --all-features, cargo test --all-features, and git diff --check.

* feat: add async commit builder ergonomics (by @patrickleet)

  Add async commit builder entrypoints for read-model, outbox, and aggregate staging so async SQL repositories can use repo.read_models(plan).commit(&mut aggregate).await.

  Update SQLite/Postgres tests and docs to exercise the direct async read-model plus aggregate transaction shape.

* test: add async distributed read model flow (by @patrickleet)

  Implements [[tasks/async-distributed-read-model-tests]]

* test: isolate postgres integration schemas (by @patrickleet)

  Fixes [[postgres-read-model-schema-bootstrap-order]]

* fix: guard bomberman spawn lookup (by @patrickleet)

* docs: rebuild async read model plan example (by @patrickleet)

* refactor: rename async commit builder starters (by @patrickleet)

* ci: pin pr postgres test actions (by @patrickleet)

* ci: pin main postgres test actions (by @patrickleet)

* refactor: make async commit builder names primary (by @patrickleet)


See full diff: [v0.5.0...v0.6.0](https://github.com/patrickleet/sourced_rust/compare/v0.5.0...v0.6.0)
