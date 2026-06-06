### What's changed in v1.5.1

* chore: Add renovate.json (#52) (by @renovate[bot])

  Co-authored-by: renovate[bot] <29139614+renovate[bot]@users.noreply.github.com>

* chore(deps): update apache/kafka docker tag to v3.9.2 (#55) (by @renovate[bot])

  Co-authored-by: renovate[bot] <29139614+renovate[bot]@users.noreply.github.com>

* chore(deps): update nats docker tag to v2.14 (#56) (by @renovate[bot])

  Co-authored-by: renovate[bot] <29139614+renovate[bot]@users.noreply.github.com>

* chore(deps): update unbounded-tech/workflow-vnext-tag action to v1.21.3 (#58) (by @renovate[bot])

  Co-authored-by: renovate[bot] <29139614+renovate[bot]@users.noreply.github.com>

* chore(deps): update actions/checkout action to v6 (#66) (by @renovate[bot])

  Co-authored-by: renovate[bot] <29139614+renovate[bot]@users.noreply.github.com>

* chore(deps): update apache/kafka docker tag to v4 (#67) (by @renovate[bot])

  Co-authored-by: renovate[bot] <29139614+renovate[bot]@users.noreply.github.com>

* chore(deps): update codecov/codecov-action action to v6 (#68) (by @renovate[bot])

  Co-authored-by: renovate[bot] <29139614+renovate[bot]@users.noreply.github.com>

* chore(deps): update github artifact actions to v7 (#69) (by @renovate[bot])

  Co-authored-by: renovate[bot] <29139614+renovate[bot]@users.noreply.github.com>

* chore(deps): update postgres docker tag to v18 (#70) (by @renovate[bot])

  Co-authored-by: renovate[bot] <29139614+renovate[bot]@users.noreply.github.com>

* chore(deps): update rabbitmq docker tag to v4 (#71) (by @renovate[bot])

  Co-authored-by: renovate[bot] <29139614+renovate[bot]@users.noreply.github.com>

* docs: correct outbox claim timing back to in-transaction (#73) (by @patrickleet)

  The PR #53 review (6885393) reworded the with_bus outbox path to say the row is
  claimed *post-commit*. That described OutboxDispatcher::dispatch_ids — a separate
  primitive used only by transport conformance tests. The actual with_bus path
  (AggregateCommit::commit) claims each row in the commit transaction via claim_at
  before commit_batch writes it (born InFlight under a short lease), then publishes
  via publish_claimed after commit. Restore the accurate in-transaction wording in
  the README and the with_bus doc comment.

  Implements [[tasks/distributed-tooling-crate-extraction]]

  Co-authored-by: Claude Opus 4.8 <noreply@anthropic.com>


See full diff: [v1.5.0...v1.5.1](https://github.com/hops-ops/distributed/compare/v1.5.0...v1.5.1)
