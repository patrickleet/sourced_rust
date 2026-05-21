### What's changed in v0.1.1

* fix: return in-memory queue lock poisoning errors (#16) (by @patrickleet)

  * fix: return in-memory queue lock poisoning errors

  Implements [[tasks/remove-in-memory-queue-lock-unwraps]]

  * test: cover in-memory poison recovery

  * fix: clear point-to-point queue state


See full diff: [v0.1.0...v0.1.1](https://github.com/patrickleet/sourced_rust/compare/v0.1.0...v0.1.1)
