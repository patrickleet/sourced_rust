-- Durable idempotency ledger for typed commands. Retention compaction turns a
-- row into a small permanent `expired` tombstone; command identities are never
-- deleted or reusable.
CREATE TABLE IF NOT EXISTS command_ledger (
  service_id text NOT NULL,
  principal_partition text NOT NULL,
  command_id text NOT NULL,
  command_name text NOT NULL,
  command_contract_hash bytea NOT NULL,
  input_hash bytea NOT NULL,
  state text NOT NULL,
  causation_id text NOT NULL,
  attempt_token text,
  attempt_number bigint NOT NULL,
  lease_expires_at timestamptz,
  outcome jsonb,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  completed_at timestamptz,
  retention_expires_at timestamptz NOT NULL,
  compacted_at timestamptz,
  PRIMARY KEY (service_id, principal_partition, command_id),
  UNIQUE (service_id, causation_id),
  CHECK (service_id <> ''),
  CHECK (principal_partition <> ''),
  CHECK (command_id <> ''),
  CHECK (command_name <> ''),
  CHECK (octet_length(command_contract_hash) = 32),
  CHECK (octet_length(input_hash) = 32),
  CHECK (causation_id <> ''),
  CHECK (attempt_number > 0),
  CHECK (state IN (
    'in_progress',
    'retryable_unknown',
    'succeeded',
    'succeeded_pending_projection',
    'projected',
    'rejected',
    'projection_failed',
    'expired'
  )),
  CHECK (
    (state = 'in_progress'
      AND attempt_token IS NOT NULL
      AND lease_expires_at IS NOT NULL
      AND outcome IS NULL
      AND completed_at IS NULL
      AND compacted_at IS NULL)
    OR
    (state = 'retryable_unknown'
      AND attempt_token IS NULL
      AND lease_expires_at IS NULL
      AND outcome IS NULL
      AND completed_at IS NULL
      AND compacted_at IS NULL)
    OR
    (state IN (
        'succeeded',
        'succeeded_pending_projection',
        'projected',
        'rejected',
        'projection_failed'
      )
      AND attempt_token IS NULL
      AND lease_expires_at IS NULL
      AND outcome IS NOT NULL
      AND completed_at IS NOT NULL
      AND compacted_at IS NULL)
    OR
    (state = 'expired'
      AND attempt_token IS NULL
      AND lease_expires_at IS NULL
      AND outcome IS NULL
      AND compacted_at IS NOT NULL)
  )
);

CREATE INDEX IF NOT EXISTS command_ledger_retention_idx
  ON command_ledger (retention_expires_at)
  WHERE state <> 'expired';
