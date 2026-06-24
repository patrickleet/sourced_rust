### What's changed in v2.2.0

* feat: Make emitter feature opt-in (#97) (by @patrickleet)

  Make emitter opt-in by removing it from the default Cargo feature set.

  Emitter-specific integration coverage is now gated behind the `emitter` feature, while the macro compile-fail fixture uses local dummy types so `distributed_macros` does not need to enable root-crate emitter APIs just to run trybuild tests. README feature and test guidance now reflects the opt-in behavior.


See full diff: [v2.1.0...v2.2.0](https://github.com/hops-ops/distributed/compare/v2.1.0...v2.2.0)
