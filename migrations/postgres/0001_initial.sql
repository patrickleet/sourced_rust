CREATE TABLE IF NOT EXISTS aggregate_events (
  aggregate_type text NOT NULL,
  aggregate_id text NOT NULL,
  sequence bigint NOT NULL,
  event_name text NOT NULL,
  event_version integer NOT NULL DEFAULT 1,
  payload bytea NOT NULL,
  payload_codec text NOT NULL,
  payload_codec_version integer NOT NULL,
  metadata jsonb NOT NULL DEFAULT '{}'::jsonb,
  recorded_at timestamptz NOT NULL DEFAULT now(),
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
  aggregate_type text NOT NULL,
  aggregate_id text NOT NULL,
  version bigint NOT NULL,
  data bytea NOT NULL,
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (aggregate_type, aggregate_id),
  CHECK (aggregate_type <> ''),
  CHECK (aggregate_id <> ''),
  CHECK (version > 0)
);
