### What's changed in v0.1.6

* chore: remove anti-pattern tests (#31) (by @patrickleet)

  these methods all require hydrating many entities and then queries them and aren't the intended use case of event sourcing repositories, and will be removed

* fix: remove repository predicate APIs (by @patrickleet)

* test: remove unused saga reservation type (by @patrickleet)

* chore: publish macros from workspace (by @patrickleet)

* docs: clarify repository trait contract (by @patrickleet)


See full diff: [v0.1.5...v0.1.6](https://github.com/patrickleet/sourced_rust/compare/v0.1.5...v0.1.6)
