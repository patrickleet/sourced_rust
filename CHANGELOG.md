### What's changed in v2.2.2

* fix: harden distributed_macros diagnostics and expand compile-fail suite (#102) (by @patrickleet)

  * fix: reject #[event] names that collide on the derived enum variant

  `event_variant_ident` PascalCases only the last `.`-segment of the event
  name, so `#[event("user.completed")]` and `#[event("admin.completed")]` in
  one #[sourced] impl both derived the variant `Completed`. The existing
  duplicate check compared full event-name strings, so the collision slipped
  through and surfaced as a duplicate-variant compile error inside generated
  code. Track the derived variant idents too and emit a spanned error naming
  both event strings at the offending attribute.

  Also narrows `seen_events` to a HashSet since its span value was unused.

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01DzYSVLas93c7LbgHJWsW7n

  * fix: error on non-identifier parameter patterns in event macros

  `extract_param_names`/`extract_params_with_types` silently filter_mapped
  away any parameter that was not a plain `Pat::Ident` (tuple patterns,
  wildcards, struct destructuring). The parameter vanished from the recorded
  payload, and the generated replay arm called the method with too few
  arguments — an arity error in generated code, far from the cause.

  Merge the two helpers into one fallible `extract_params_with_types` that
  emits a spanned 'unsupported parameter pattern in #[event|digest|enqueue]
  method — use a plain identifier' error at the offending pattern.

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01DzYSVLas93c7LbgHJWsW7n

  * refactor: parse upcaster entries with one shared UpcasterDef parser

  The upcaster grammar ('name', from => to, Src => Tgt, transform_fn) was
  parsed twice — once in AggregateInput::parse and once in parse_sourced_args
  — so the two copies could drift. Extract an impl Parse for UpcasterDef plus
  a parse_upcaster_list helper used by both entry points. Grammar is
  unchanged, including optional trailing commas between entries.

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01DzYSVLas93c7LbgHJWsW7n

  * test: cover derive and function-like macro misuse in compile-fail suite

  Extends the trybuild suite beyond attribute macros:

  - derive(ReadModel): missing id field / #[readmodel(id)] marker, and an
    unknown readmodel field attribute key
  - derive(Snapshot): missing entity field (with the available-fields hint)
    and tuple-struct rejection
  - aggregate!: unknown keyword argument in place of aggregate_type

  Each fixture pins its .stderr so diagnostic quality regressions are caught.
  All diagnostics fire during attribute parsing, before codegen, so they are
  independent of the schema() codegen changes landing separately.

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01DzYSVLas93c7LbgHJWsW7n

  ---------

  Co-authored-by: Claude Fable 5 <noreply@anthropic.com>


See full diff: [v2.2.1...v2.2.2](https://github.com/hops-ops/distributed/compare/v2.2.1...v2.2.2)
