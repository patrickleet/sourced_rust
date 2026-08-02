-- SQLite cannot ALTER CHECK constraints in place. Rebuild command_ledger with
-- atomic as the same-tx terminal state (was projected).

CREATE TABLE command_ledger_atomic (
  service_id TEXT NOT NULL,
  principal_partition TEXT NOT NULL,
  command_id TEXT NOT NULL,
  command_name TEXT NOT NULL,
  command_contract_hash BLOB NOT NULL,
  input_hash BLOB NOT NULL,
  state TEXT NOT NULL,
  causation_id TEXT NOT NULL,
  attempt_token TEXT,
  attempt_number INTEGER NOT NULL,
  lease_expires_at REAL,
  outcome TEXT,
  created_at REAL NOT NULL DEFAULT (unixepoch('now','subsec')),
  updated_at REAL NOT NULL DEFAULT (unixepoch('now','subsec')),
  completed_at REAL,
  retention_expires_at REAL NOT NULL,
  compacted_at REAL,
  PRIMARY KEY (service_id, principal_partition, command_id),
  UNIQUE (service_id, causation_id),
  CHECK (service_id <> ''),
  CHECK (principal_partition <> ''),
  CHECK (command_id <> ''),
  CHECK (command_name <> ''),
  CHECK (typeof(command_contract_hash) = 'blob' AND length(command_contract_hash) = 32),
  CHECK (typeof(input_hash) = 'blob' AND length(input_hash) = 32),
  CHECK (causation_id <> ''),
  CHECK (attempt_number > 0),
  CHECK (outcome IS NULL OR json_valid(outcome)),
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

INSERT INTO command_ledger_atomic (
  service_id,
  principal_partition,
  command_id,
  command_name,
  command_contract_hash,
  input_hash,
  state,
  causation_id,
  attempt_token,
  attempt_number,
  lease_expires_at,
  outcome,
  created_at,
  updated_at,
  completed_at,
  retention_expires_at,
  compacted_at
)
SELECT
  service_id,
  principal_partition,
  command_id,
  command_name,
  command_contract_hash,
  input_hash,
  CASE state WHEN 'projected' THEN 'atomic' ELSE state END,
  causation_id,
  attempt_token,
  attempt_number,
  lease_expires_at,
  outcome,
  created_at,
  updated_at,
  completed_at,
  retention_expires_at,
  compacted_at
FROM command_ledger;
DROP TABLE command_ledger;
ALTER TABLE command_ledger_atomic RENAME TO command_ledger;

CREATE INDEX IF NOT EXISTS command_ledger_retention_idx
  ON command_ledger (retention_expires_at)
  WHERE state <> 'expired';
