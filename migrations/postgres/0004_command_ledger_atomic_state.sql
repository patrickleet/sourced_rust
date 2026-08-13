-- Rename terminal command-ledger state projected → atomic (aggregate+read-model
-- same-tx completion). Drop CHECKs first, rewrite rows, then re-add CHECKs.

DO $$
DECLARE
  r RECORD;
BEGIN
  FOR r IN
    SELECT c.conname
    FROM pg_constraint c
    WHERE c.conrelid = 'command_ledger'::regclass
      AND c.contype = 'c'
      AND (
        pg_get_constraintdef(c.oid) LIKE '%projected%'
        OR pg_get_constraintdef(c.oid) LIKE '%state IN%'
      )
  LOOP
    EXECUTE format('ALTER TABLE command_ledger DROP CONSTRAINT %I', r.conname);
  END LOOP;
END $$;

UPDATE command_ledger
SET state = 'atomic'
WHERE state = 'projected';

ALTER TABLE command_ledger
  ADD CONSTRAINT command_ledger_state_values_check CHECK (state IN (
    'in_progress',
    'retryable_unknown',
    'succeeded',
    'succeeded_pending_projection',
    'atomic',
    'rejected',
    'projection_failed',
    'expired'
  ));

ALTER TABLE command_ledger
  ADD CONSTRAINT command_ledger_state_shape_check CHECK (
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
  );
