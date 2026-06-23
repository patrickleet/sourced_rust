### What's changed in v2.1.0

* feat: Implement typed microsvc route bundles (#95) (by @patrickleet)

  * Implement typed microsvc route bundles
  * Remove legacy register_handlers macro
  * Add routes dependency combination coverage

  This is a breaking change, use routes! macro instead of register_handlers. Services are not sets of route bundles to support multiple aggregates/repos per service.


See full diff: [v2.0.0...v2.1.0](https://github.com/hops-ops/distributed/compare/v2.0.0...v2.1.0)
