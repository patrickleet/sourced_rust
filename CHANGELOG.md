### What's changed in v3.3.4

* refactor: make Session identity claims gateway-agnostic (by @patrickleet)

  Hasura was only an example query layer, but convenience helpers and docs
  treated x-hasura-* as the protocol. Use neutral x-user-id/x-role keys,
  document Session as an opaque claim map, and keep gateway-specific names
  readable via Session::get.


See full diff: [v3.3.3...v3.3.4](https://github.com/hops-ops/distributed/compare/v3.3.3...v3.3.4)
