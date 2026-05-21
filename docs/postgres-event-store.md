# Postgres Event Store Contract

This is the storage contract for the future Postgres repository. It narrows the durable representation without requiring the Postgres implementation to land in the same change.

## Event Records

The event table stores aggregate-sourced replay records. Published domain events and integration messages should use the outbox/message tables.

Recommended table shape:

| Column | Type | Notes |
| --- | --- | --- |
| `aggregate_type` | `text` | Stable aggregate type name. |
| `aggregate_id` | `text` | Aggregate stream identifier. |
| `sequence` | `bigint` | One-based stream position. |
| `event_name` | `text` | Stable replay event name. |
| `event_version` | `bigint` | Payload schema version, default `1`. |
| `payload` | `bytea` | Raw encoded event payload bytes. |
| `payload_codec` | `text` | Required codec label, initially `bitcode`. |
| `payload_codec_version` | `integer` | Required codec metadata, initially `1`. |
| `metadata` | `jsonb` | Event metadata, default `{}`. |
| `recorded_at` | `timestamptz` | UTC instant for the event record. |

Required constraints and indexes:

- `PRIMARY KEY (aggregate_type, aggregate_id, sequence)`.
- Optional index `(event_name, event_version)` for migrations or diagnostics.

## Timestamp Representation

Rust `EventRecord::timestamp` remains `SystemTime` in the in-memory API, but Postgres must not persist serde's `SystemTime` JSON shape. The database representation is `recorded_at timestamptz NOT NULL`, bound as a UTC instant. Implementations should round or truncate to the database/driver's supported precision, normally microseconds, and convert back to `SystemTime` at the repository boundary.

## Payload Codec Metadata

Event payload bytes are currently bitcode-encoded. Postgres rows must carry codec metadata beside the payload so future codecs or bitcode compatibility changes can be handled explicitly:

- `payload_codec = 'bitcode'`
- `payload_codec_version = 1`

The repository should reject unknown codec labels or versions unless an explicit decoder/upcaster path exists. Payload schema changes still use `event_version` and aggregate upcasters; codec metadata describes the byte encoding, not the domain event version.

## Locking Model

`QueuedRepository` remains a process-local coordination wrapper for examples, tests, and single-process adapters. It complements a Postgres repository but must not be the durable cross-process locking mechanism.

The Postgres repository should enforce optimistic concurrency with the `(aggregate_type, aggregate_id, sequence)` uniqueness constraint. If a queued read/modify/write API is exposed for Postgres, it should use database-backed row locks or advisory locks inside the repository/transaction boundary. Those database locks replace process-local queue locks for cross-process writer coordination; `QueuedRepository` can still wrap a Postgres repository only as an additional in-process convenience layer.

## Backward Compatibility

Rows or imported JSON records without event metadata deserialize with empty metadata. Postgres migrations should still write `metadata jsonb NOT NULL DEFAULT '{}'` so newly stored rows are explicit.
