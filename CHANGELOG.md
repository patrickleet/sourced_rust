### What's changed in v2.3.1

* refactor: dedupe snapshot tail replay into one iterator-based fn (by @patrickleet)

  replay_filtered and replay_events were near-duplicates; one replay_events
  taking impl Iterator<Item = &EventRecord> now serves both the borrowed
  post-snapshot tail and the upcasted copy, with post_snapshot_tail naming
  the shared filter.

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01DzYSVLas93c7LbgHJWsW7n

* perf: skip event cloning when no upcaster applies during hydrate (by @patrickleet)

  Registering any upcaster used to deep-clone the whole event history on
  every hydrate (and the post-snapshot tail on snapshot loads), even when
  all events were already current. upcast_events_for_replay now pre-scans
  the borrowed view: if no (event_type, from_version) matches, replay runs
  straight off the borrowed events; if some match, only those events are
  cloned (per-event Cow). upcast_one also defers its cycle-tracking HashSet
  until the first transform applies.

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01DzYSVLas93c7LbgHJWsW7n

* perf: stop cloning all five HashMapRepository stores per commit (by @patrickleet)

  commit_batch cloned the entire event/read-model/snapshot/outbox/inbox
  maps on every commit to get all-or-nothing semantics, which grows
  quadratic over a session. Fallible steps now run first — event version
  checks and outbox/inbox duplicate checks read the live maps; read-model
  plans (the only step that can fail mid-application) apply to a staging
  copy bounded to the rows the batch touches (every touched storage key is
  known up front via the mutations' lock keys) — and the live maps are
  mutated only once nothing can fail. Guards are still held throughout so
  the batch stays atomic to readers.

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01DzYSVLas93c7LbgHJWsW7n

* perf: evict idle InMemoryLockManager entries on unlock (by @patrickleet)

  The manager kept one Arc<InMemoryLock> per aggregate id ever locked,
  forever. A manager-owned lock now carries a weak back-reference to the
  manager's map and evicts its entry on unlock when, re-checked under the
  map guard, only the map's Arc and the unlocking handle remain and the
  lock is idle (unlocked, no waiters). Parked waiters hold live Arcs, so
  their entries survive. get_lock documents that handles are per
  acquire/release cycle, matching QueuedRepository's usage.

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01DzYSVLas93c7LbgHJWsW7n

* fix: surface JSON parse errors instead of nulling handler input (by @patrickleet)

  dispatch_message swallowed message_to_json_input failures and dispatched
  with Value::Null input, discarding the parse error. The Null fallback is
  intentional only for non-JSON payloads (bitcode/binary), whose handlers
  read the raw payload; for JSON content types a malformed payload now
  returns HandlerError::DecodeFailed carrying the parse failure.

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01DzYSVLas93c7LbgHJWsW7n

* perf: cut per-dispatch allocations in Service and Handlers routing (by @patrickleet)

  - Context now borrows the Message instead of owning a clone, so
    dispatch_message no longer clones the whole message (payload included)
    per dispatch.
  - Handler maps are keyed HashMap<MessageKind, HashMap<String, _>> in
    Routes, Service, and bus::Handlers, so lookups take &str and no longer
    allocate a (kind, String) key per dispatch/handles check.

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01DzYSVLas93c7LbgHJWsW7n

* fix: return an error from Service::run without a bus; assert entity invariant (by @patrickleet)

  Service::run already returns Result<(), TransportError>, so a missing
  bus is now a permanent TransportError pointing at with_bus instead of a
  panic. Entity::new_events also debug_asserts the
  committed_version >= prefix_version invariant established by
  load_tail_from_history, so a violated invariant fails at the source
  instead of as a wrapped subtraction.

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01DzYSVLas93c7LbgHJWsW7n

* refactor: collapse CommitBuilder and StagedCommitBuilder into one builder (by @patrickleet)

  CommitBuilder and StagedCommitBuilder duplicated read_models/outbox
  staging and CommitBatch construction, and outbox source stamping
  differed by entry path: commit(agg) stamped every message, staged
  commits stamped only an unambiguous source, and commit_many stamped
  nothing. All behavior now lives on CommitBuilder (staging state is the
  stream writes plus StagedOutboxSource); StagedCommitBuilder is a
  transparent naming state that only exists to offer the no-argument
  commit(). One documented stamping rule everywhere: exactly one distinct
  staged aggregate stamps the source, otherwise messages keep what they
  carry. commit(agg) is now sugar for aggregate(agg).commit(). The private
  OutboxSource struct is renamed SourceStamp so it no longer shadows the
  public outbox_worker::OutboxSource.

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01DzYSVLas93c7LbgHJWsW7n

* style: use arrays for borrowed event fixtures in upcaster tests (by @patrickleet)

  Clippy (useless_vec): the new upcast_events_for_replay tests only
  iterate borrowed events, so plain arrays suffice.

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01DzYSVLas93c7LbgHJWsW7n

* test: label bitcode outbox payloads as octet-stream in board example (by @patrickleet)

  The board example's outbox bridge forwarded raw bitcode bytes with
  Message::new's default application/json content type; the projection
  only worked because JSON parse failures were silently nulled. Now that
  malformed JSON surfaces as a decode error, label the binary payload the
  way the real outbox dispatch does (application/octet-stream).

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01DzYSVLas93c7LbgHJWsW7n


See full diff: [v2.3.0...v2.3.1](https://github.com/hops-ops/distributed/compare/v2.3.0...v2.3.1)
