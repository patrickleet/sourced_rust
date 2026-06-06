### What's changed in v1.6.3

* docs: document the dsvc CLI (scaffold/describe/schema + schema --format atlas) (#75) (by @patrickleet)

  Add a crate README for distributed_cli (shows on crates.io) covering install, the
  distributed_manifest() entrypoint, and `dsvc schema` SQL/Atlas output with the
  AtlasSchema flag reference and example. Wire it via `readme` in Cargo.toml. Add a
  concise "Service CLI (dsvc)" section to the main README linking to it.

  Implements [[tasks/atlas-operator-schema-gitops]]

  Co-authored-by: Claude Opus 4.8 <noreply@anthropic.com>


See full diff: [v1.6.2...v1.6.3](https://github.com/hops-ops/distributed/compare/v1.6.2...v1.6.3)
