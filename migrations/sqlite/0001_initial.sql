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
