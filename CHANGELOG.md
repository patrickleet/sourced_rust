### What's changed in v2.0.0

* chore(deps): update actions/checkout action to v7 (#92) (by @renovate[bot])

  Co-authored-by: renovate[bot] <29139614+renovate[bot]@users.noreply.github.com>

* chore(docs): sqlitebus (by @patrickleet)

* feat: remove redundant async API names (#94) (by @patrickleet)

  BREAKING CHANGE: Remove redundant `Async` prefixes from the public API now that the crate is async-first.

    Renames:
    - `AsyncMessageSource` -> `MessageSource`
    - `AsyncMessagePublisher` -> `MessagePublisher`
    - `AsyncMessageHandler` -> `MessageHandler`
    - `AsyncLock` -> `Lock`
    - `AsyncLockManager` -> `LockManager`
    - `InMemoryAsyncLock` -> `InMemoryLock`
    - `InMemoryAsyncLockFuture` -> `InMemoryLockFuture`
    - `InMemoryAsyncLockManager` -> `InMemoryLockManager`

    Also renames the lock implementation modules from `async_*` to neutral module names and moves the transport guide from `docs/async-transports.md` to `docs/transports.md`.

    This is a breaking API cleanup; no compatibility aliases are kept.


See full diff: [v1.9.0...v2.0.0](https://github.com/hops-ops/distributed/compare/v1.9.0...v2.0.0)
