### What's changed in v1.7.0

* chore(deps): update codecov/codecov-action action to v7 (#76) (by @renovate[bot])

  Co-authored-by: renovate[bot] <29139614+renovate[bot]@users.noreply.github.com>

* feat: infer and harden bus consumer topology (#77) (by @patrickleet)

  * feat: infer and harden bus consumer topology

  Adds Service::named-derived consumer groups, awaitable bus constructors, shared topology validation, and inferred-group transport coverage.

  Implements [[tasks/infer-bus-topology-from-service-name]] and [[tasks/harden-inferred-bus-topology]].

  * test: cover named handler consumer group

  Addresses CodeRabbit review on [[tasks/address-coderabbit-inferred-bus-topology]].


See full diff: [v1.6.3...v1.7.0](https://github.com/hops-ops/distributed/compare/v1.6.3...v1.7.0)
