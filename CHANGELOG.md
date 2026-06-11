### What's changed in v1.7.6

* fix: harden transport and HTTP ingress against hostile input (S3-S6) (#84) (by @patrickleet)

  Security hardening batch for the Knative/HTTP ingress, bus message-name
  routing, consumer inbox growth, generated DDL defaults, and request body
  limits. Excludes the gRPC error-masking item (owned by a separate PR).

  S3 — Knative ingress error leak: the CloudEvents ingress returned
  `err.to_string()` verbatim, which can carry SQL/driver/path detail.
  Internal faults are now masked to "Internal server error" (and logged
  server-side) by reusing a shared `HandlerError::redacted_message()`
  helper, which the HTTP ingress now also uses instead of its private
  masking fn — single source of truth.

  S4 — Message name validation: `Message.name` flows unmodified into the
  NATS subject, Kafka topic, and RabbitMQ routing/binding key but had no
  rules. Added `validate_message_name` (mirrors `validate_stable_message_id`):
  rejects empty, over-long (>256B), control-character-bearing, and
  wildcard-bearing (`*`/`#`/`>`) names; `.` stays allowed since dotted type
  names are the convention. Enforced inbound on the attacker-controlled
  `ce-type` (binary + structured) and outbound on the RabbitMQ
  send/publish/bind paths (a `#`/`*` in a binding key is a subscription
  wildcard). `.` routing semantics documented on `Message::name`.

  S5 — Unbounded inbox growth: added `InboxStore::purge_inbox_older_than`
  (Postgres/SQLite issue a DB-clock-relative bounded DELETE; HashMap is a
  documented no-op). HashMapRepository gains `clear_inbox` as the in-memory
  equivalent and its inbox is now marked dev-only in rustdoc. Retention is
  documented as the operator's responsibility.

  S6 — Raw DEFAULT in generated DDL: the table SQL generator spliced
  `column.default` unquoted — the one unescaped hole in an otherwise fully
  quoted generator. Now validated against an allowlist (numeric/boolean/
  NULL/CURRENT_TIMESTAMP keywords or a properly-escaped single-quoted
  literal); anything else fails generation loudly. Atlas consumes the
  validated output unchanged.

  Body limits: pinned axum's implicit 2MiB default to an explicit 1MiB
  `DefaultBodyLimit` on both the command router and the CloudEvents
  ingress (both buffer the whole body), via one shared `MAX_HTTP_BODY_BYTES`.

  Doc caveat: `Message::payload_bitcode` now warns bitcode is not hardened
  against hostile input — decode only from trusted producers.

  Implements [[tasks/transport-ingress-security-hardening]]

  Co-authored-by: Claude Fable 5 <noreply@anthropic.com>


See full diff: [v1.7.5...v1.7.6](https://github.com/hops-ops/distributed/compare/v1.7.5...v1.7.6)
