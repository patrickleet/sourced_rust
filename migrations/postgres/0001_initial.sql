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
  snapshot_type text NOT NULL,
  snapshot_version integer NOT NULL,
  payload bytea NOT NULL,
  payload_codec text NOT NULL,
  payload_codec_version integer NOT NULL,
  metadata jsonb NOT NULL DEFAULT '{}'::jsonb,
  recorded_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
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
  CHECK (attempts >= 0),
  CHECK (
    CASE status
      WHEN 'pending' THEN
        claimed_by IS NULL
        AND claimed_until IS NULL
        AND published_at IS NULL
        AND failed_at IS NULL
      WHEN 'in_flight' THEN
        claimed_by IS NOT NULL
        AND claimed_until IS NOT NULL
        AND published_at IS NULL
        AND failed_at IS NULL
      WHEN 'published' THEN
        claimed_by IS NULL
        AND claimed_until IS NULL
        AND published_at IS NOT NULL
        AND failed_at IS NULL
      WHEN 'failed' THEN
        claimed_by IS NULL
        AND claimed_until IS NULL
        AND published_at IS NULL
        AND failed_at IS NOT NULL
      ELSE FALSE
    END
  )
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

-- Consumer inbox: optional effectively-once effect fence (the consumer-side
-- complement to the outbox). A receipt is written in the same transaction as the
-- consumer's other writes, so a duplicate (consumer, message_id) is a no-op replay.
CREATE TABLE IF NOT EXISTS consumer_inbox (
  consumer text NOT NULL,
  message_id text NOT NULL,
  processed_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (consumer, message_id),
  CHECK (consumer <> ''),
  CHECK (message_id <> '')
);

-- Durable per-stream lease lock backing `QueuedRepository` cross-process
-- serialization (see `PostgresLockManager`). A row is a held lease: `owner_token`
-- identifies the acquiring generation and `expires_at`/`acquired_at` are epoch
-- seconds from the database clock (extract(epoch from now())). A lease is
-- stealable once expires_at is at or before now. Release is scoped to the owner
-- token so it never frees a holder that reclaimed an expired lease. This is a
-- mutual-exclusion optimization, NOT a fencing guarantee — the event-store
-- sequence primary key remains the authoritative concurrency boundary.
CREATE TABLE IF NOT EXISTS aggregate_locks (
  lock_key text NOT NULL PRIMARY KEY,
  owner_token text NOT NULL,
  acquired_at double precision NOT NULL,
  expires_at double precision NOT NULL,
  CHECK (lock_key <> ''),
  CHECK (owner_token <> '')
);

CREATE INDEX IF NOT EXISTS aggregate_locks_expires_at_idx
  ON aggregate_locks (expires_at);
