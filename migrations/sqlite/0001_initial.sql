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

CREATE TABLE IF NOT EXISTS transactional_read_models (
  collection TEXT NOT NULL,
  id TEXT NOT NULL,
  version INTEGER NOT NULL,
  payload BLOB NOT NULL,
  updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (collection, id),
  CHECK (collection <> ''),
  CHECK (id <> ''),
  CHECK (version > 0)
);

CREATE TABLE IF NOT EXISTS read_model_processed_messages (
  consumer_name TEXT NOT NULL,
  message_id TEXT NOT NULL,
  processed_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (consumer_name, message_id),
  CHECK (consumer_name <> ''),
  CHECK (message_id <> '')
);

CREATE TABLE IF NOT EXISTS aggregate_snapshots (
  aggregate_type TEXT NOT NULL,
  aggregate_id TEXT NOT NULL,
  version INTEGER NOT NULL,
  data BLOB NOT NULL,
  updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (aggregate_type, aggregate_id),
  CHECK (aggregate_type <> ''),
  CHECK (aggregate_id <> ''),
  CHECK (version > 0)
);
