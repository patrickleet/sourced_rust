### What's changed in v4.11.0

* feat: infer GraphQL island variable defaults (#221) (by @patrickleet)

  * feat!: infer GraphQL island variable defaults

  BREAKING CHANGE: generated variable codec v1 artifacts are rejected; regenerate clients with the matching framework version.

  * fix: watch GraphQL binding sidecars in dev

  * fix: bound lifecycle client input patterns

  * fix: trust lifecycle compiler staging root

  * fix: avoid supervised Vite compile race

  * fix: keep supervised Vite serving through activation

  * fix: defer celld drain after durable acceptance


See full diff: [v4.10.0...v4.11.0](https://github.com/hops-ops/distributed/compare/v4.10.0...v4.11.0)
