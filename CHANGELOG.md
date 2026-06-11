### What's changed in v1.7.4

* refactor: harden distributed_macros expansion and diagnostics (#82) (by @patrickleet)

  Restructure the four proc-macros in distributed_macros/src/lib.rs
  (sourced, aggregate!, digest, enqueue) to the testable
  `expand_* -> syn::Result<TokenStream2>` shape already used by
  snapshot.rs and read_model.rs. The thin `#[proc_macro*]` entry points
  now convert errors via `.unwrap_or_else(|e| e.to_compile_error())`.
  Generated output is byte-identical — only the structure and error
  plumbing changed (verified: all 250+ macro-using tests in the main
  crate still pass).

  Diagnostics: unknown keyword args in parse_digest_args,
  parse_sourced_args, parse_event_args, and parse_enqueue_args now
  produce pointed, spanned syn::Error messages that name the bad key and
  list the valid ones, instead of being silently left unconsumed and
  surfacing as a bare "unexpected token". Also added up-front checks for
  duplicate #[event] names and #[event] methods missing a self receiver,
  and a clearer "missing entity field" message for bare #[sourced()].

  enqueue fix: #[enqueue] now accepts `entity = field` so a renamed
  entity field produces a correct `is_replaying()` guard instead of a
  confusing "no field `entity`" error pointing at the user's method.

  ReplayError: kept as `String`. Replay errors are flattened from
  heterogeneous sources (per-event decode errors, user method errors of
  arbitrary E, unknown-event messages) via `e.to_string()`; a typed
  error would have to be generic over each method's error type or erase
  them anyway. Rationale documented inline.

  Tests: added unit tests for the new expand_*/parse_* functions and a
  trybuild compile-fail suite (tests/compile_fail/*.rs + harness)
  covering an unsupported #[event] signature, duplicate event names,
  unknown attribute keys, #[sourced] missing the entity field, and the
  renamed-entity #[enqueue] footgun.

  Implements [[tasks/macro-crate-hardening]]


See full diff: [v1.7.3...v1.7.4](https://github.com/hops-ops/distributed/compare/v1.7.3...v1.7.4)
