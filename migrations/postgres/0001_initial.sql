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

CREATE TABLE IF NOT EXISTS outbox_messages (
  message_id text NOT NULL PRIMARY KEY,
  event_type text NOT NULL,
  payload bytea NOT NULL,
  payload_codec text NOT NULL,
  payload_codec_version integer NOT NULL,
  destination text,
  metadata jsonb NOT NULL DEFAULT '{}'::jsonb,
  status text NOT NULL,
  created_at timestamptz NOT NULL,
  next_available_at timestamptz NOT NULL,
  claimed_by text,
  claimed_until timestamptz,
  attempts integer NOT NULL DEFAULT 0,
  last_error text,
  published_at timestamptz,
  failed_at timestamptz,
  source_aggregate_type text,
  source_aggregate_id text,
  source_sequence bigint,
  correlation_id text,
  causation_id text,
  updated_at timestamptz NOT NULL DEFAULT now(),
  CHECK (message_id <> ''),
  CHECK (event_type <> ''),
  CHECK (payload_codec <> ''),
  CHECK (payload_codec_version > 0),
  CHECK (status IN ('pending', 'in_flight', 'published', 'failed')),
  CHECK (attempts >= 0)
);

CREATE INDEX IF NOT EXISTS outbox_messages_pending_idx
  ON outbox_messages (created_at)
  WHERE status = 'pending';

CREATE INDEX IF NOT EXISTS outbox_messages_in_flight_expiry_idx
  ON outbox_messages (claimed_until, created_at)
  WHERE status = 'in_flight';

CREATE INDEX IF NOT EXISTS outbox_messages_source_idx
  ON outbox_messages (source_aggregate_type, source_aggregate_id, source_sequence);

CREATE INDEX IF NOT EXISTS outbox_messages_destination_idx
  ON outbox_messages (destination, status, created_at);
