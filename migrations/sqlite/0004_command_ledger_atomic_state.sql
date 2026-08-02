-- SQLite cannot ALTER CHECK constraints in place. Rebuild command_ledger with
-- atomic as the same-tx terminal state (was projected).

UPDATE command_ledger
SET state = 'atomic'
WHERE state = 'projected';

CREATE TABLE command_ledger_atomic (
  service_id text NOT NULL,
  principal_partition text NOT NULL,
  command_id text NOT NULL,
  command_name text NOT NULL,
  command_contract_hash blob NOT NULL,
  input_hash blob NOT NULL,
  state text NOT NULL,
  causation_id text NOT NULL,
  attempt_token text,
  attempt_number integer NOT NULL,
  lease_expires_at text,
  outcome text,
  created_at text NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
  updated_at text NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
  completed_at text,
  retention_expires_at text NOT NULL,
  compacted_at text,
  PRIMARY KEY (service_id, principal_partition, command_id),
  UNIQUE (service_id, causation_id),
  CHECK (service_id <> ''),
  CHECK (principal_partition <> ''),
  CHECK (command_id <> ''),
  CHECK (command_name <> ''),
  CHECK (length(command_contract_hash) = 32),
  CHECK (length(input_hash) = 32),
  CHECK (causation_id <> ''),
  CHECK (attempt_number > 0),
  CHECK (state IN (
    'in_progress',
    'retryable_unknown',
    'succeeded',
    'succeeded_pending_projection',
    'atomic',
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
        'atomic',
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

INSERT INTO command_ledger_atomic SELECT * FROM command_ledger;
DROP TABLE command_ledger;
ALTER TABLE command_ledger_atomic RENAME TO command_ledger;

CREATE INDEX IF NOT EXISTS command_ledger_retention_idx
  ON command_ledger (retention_expires_at)
  WHERE state <> 'expired';
