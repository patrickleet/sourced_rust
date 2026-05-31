CREATE TABLE IF NOT EXISTS aggregate_events (
  aggregate_type TEXT NOT NULL,
  aggregate_id TEXT NOT NULL,
  sequence INTEGER NOT NULL,
  event_name TEXT NOT NULL,
  event_version INTEGER NOT NULL DEFAULT 1,
  payload BLOB NOT NULL,
  payload_codec TEXT NOT NULL,
  payload_codec_version INTEGER NOT NULL,
  metadata TEXT NOT NULL DEFAULT '{}',
  recorded_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (aggregate_type, aggregate_id, sequence),
  CHECK (aggregate_type <> ''),
  CHECK (aggregate_id <> ''),
  CHECK (sequence > 0),
  CHECK (event_version > 0),
  CHECK (payload_codec <> ''),
  CHECK (payload_codec_version > 0)
);

CREATE INDEX IF NOT EXISTS aggregate_events_event_version_idx
  ON aggregate_events (aggregate_type, event_name, event_version);

CREATE INDEX IF NOT EXISTS aggregate_events_recorded_at_idx
  ON aggregate_events (recorded_at);

CREATE TABLE IF NOT EXISTS aggregate_snapshots (
  aggregate_type TEXT NOT NULL,
  aggregate_id TEXT NOT NULL,
  version INTEGER NOT NULL,
  snapshot_type TEXT NOT NULL,
  snapshot_version INTEGER NOT NULL,
  payload BLOB NOT NULL,
  payload_codec TEXT NOT NULL,
  payload_codec_version INTEGER NOT NULL,
  metadata TEXT NOT NULL DEFAULT '{}',
  recorded_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (aggregate_type, aggregate_id),
  CHECK (aggregate_type <> ''),
  CHECK (aggregate_id <> ''),
  CHECK (version > 0),
  CHECK (snapshot_type <> ''),
  CHECK (snapshot_version > 0),
  CHECK (payload_codec <> ''),
  CHECK (payload_codec_version > 0)
);

CREATE TABLE IF NOT EXISTS outbox_messages (
  message_id TEXT NOT NULL PRIMARY KEY,
  event_type TEXT NOT NULL,
  payload BLOB NOT NULL,
  payload_codec TEXT NOT NULL,
  payload_codec_version INTEGER NOT NULL,
  destination TEXT,
  metadata TEXT NOT NULL DEFAULT '{}',
  status TEXT NOT NULL,
  created_at TEXT NOT NULL,
  next_available_at TEXT NOT NULL,
  claimed_by TEXT,
  claimed_until TEXT,
  attempts INTEGER NOT NULL DEFAULT 0,
  last_error TEXT,
  published_at TEXT,
  failed_at TEXT,
  source_aggregate_type TEXT,
  source_aggregate_id TEXT,
  source_sequence INTEGER,
  correlation_id TEXT,
  causation_id TEXT,
  updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  CHECK (message_id <> ''),
  CHECK (event_type <> ''),
  CHECK (payload_codec <> ''),
  CHECK (payload_codec_version > 0),
  CHECK (status IN ('pending', 'in_flight', 'published', 'failed')),
  CHECK (attempts >= 0)
);

CREATE INDEX IF NOT EXISTS outbox_messages_claimable_idx
  ON outbox_messages (status, next_available_at, claimed_until, created_at);

CREATE INDEX IF NOT EXISTS outbox_messages_source_idx
  ON outbox_messages (source_aggregate_type, source_aggregate_id, source_sequence);

CREATE INDEX IF NOT EXISTS outbox_messages_destination_idx
  ON outbox_messages (destination, status, created_at);

-- Consumer inbox: optional effectively-once effect fence (the consumer-side
-- complement to the outbox). A receipt is written in the same transaction as the
-- consumer's other writes, so a duplicate (consumer, message_id) is a no-op replay.
CREATE TABLE IF NOT EXISTS consumer_inbox (
  consumer TEXT NOT NULL,
  message_id TEXT NOT NULL,
  processed_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (consumer, message_id),
  CHECK (consumer <> ''),
  CHECK (message_id <> '')
);

-- Durable per-stream lease lock backing `QueuedRepository` cross-process
-- serialization (see `SqliteLockManager`). A row is a held lease: `owner_token`
-- identifies the acquiring generation and `expires_at`/`acquired_at` are epoch
-- seconds from the database clock (unixepoch('now','subsec')). A lease is
-- stealable once expires_at is at or before now. Release is scoped to the owner
-- token so it never frees a holder that reclaimed an expired lease. This is a
-- mutual-exclusion optimization, NOT a fencing guarantee — the event-store
-- sequence primary key remains the authoritative concurrency boundary.
CREATE TABLE IF NOT EXISTS aggregate_locks (
  lock_key TEXT NOT NULL PRIMARY KEY,
  owner_token TEXT NOT NULL,
  acquired_at REAL NOT NULL,
  expires_at REAL NOT NULL,
  CHECK (lock_key <> ''),
  CHECK (owner_token <> '')
);

CREATE INDEX IF NOT EXISTS aggregate_locks_expires_at_idx
  ON aggregate_locks (expires_at);
