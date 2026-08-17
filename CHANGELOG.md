### What's changed in v4.1.1

* docs: rewrite README from the e2e-ui home story (#201) (by @patrickleet)

  * docs: rewrite README from the e2e-ui home story

  Lead with the playground teaching path — the bar, backstory, and the
  unidirectional loop from command to replica — instead of a crate-tour
  quick start. Keep the API reference from Feature Flags onward.

  Implements [[tasks/service-authoring-1]]

  * chore: Improve formatting of Todos query in README

  Format the Todos query for better readability.

  * docs: say generated GraphQL, replica cache, and WASM pures out loud

  Home page and README now state that Rust definitions generate the GraphQL
  schema and typed client; auto-optimism applies the projection mutation to
  a client replica cache (same program as the SQL projector); and advanced
  cases ship the domain pure as WASM for the generated host.

  Implements [[tasks/service-authoring-1]]

  * docs: say it is an end-to-end framework and a toolkit

  The playground is the full path. The same crates stay usable a la carte —
  aggregates only, bus only, GraphQL without the replica. Drop the old
  "not a toolkit" line that fought that adoption model.

  Implements [[tasks/service-authoring-1]]

  * docs: fix README snippet languages and compose accuracy

  Split the CQRS example into ts and graphql fences. Label the todos
  projection as abbreviated and add epoch plus the crate-root macro.
  Show the real routes(...) inputs from compose.rs.

  Implements [[tasks/service-authoring-1]]


See full diff: [v4.1.0...v4.1.1](https://github.com/hops-ops/distributed/compare/v4.1.0...v4.1.1)
