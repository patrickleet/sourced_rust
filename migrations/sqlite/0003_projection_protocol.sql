-- Framework-owned causal projection protocol metadata.
--
-- This additive pre-release migration deliberately performs no backfill:
-- existing read-model rows have no inferred causation, revision, checkpoint,
-- observation, or ownership evidence. Adoption requires a rebuild into a new
-- projection epoch or a separately verified importer.

CREATE TABLE IF NOT EXISTS projection_partitions (
  topology_bytes BLOB NOT NULL,
  topology_hash BLOB NOT NULL,
  partition_bytes BLOB NOT NULL,
  partition_hash BLOB NOT NULL,
  active_generation INTEGER NOT NULL DEFAULT 1,
  change_epoch TEXT NOT NULL,
  change_head INTEGER NOT NULL DEFAULT 0,
  compacted_through INTEGER NOT NULL DEFAULT 0,
  pending_retry_failure_id TEXT,
  stopped_failure_id TEXT,
  stopped_source_bytes BLOB,
  stopped_source_hash BLOB,
  stopped_source_partition_bytes BLOB,
  stopped_source_partition_hash BLOB,
  stopped_source_epoch TEXT,
  stopped_source_position INTEGER,
  stopped_generation INTEGER,
  stopped_input_hash BLOB,
  stopped_message_id TEXT,
  stopped_causation_id TEXT,
  stopped_gap_free INTEGER,
  PRIMARY KEY (topology_hash, partition_hash),
  CHECK (typeof(topology_bytes) = 'blob'
    AND length(topology_bytes) BETWEEN 1 AND 4096),
  CHECK (typeof(topology_hash) = 'blob' AND length(topology_hash) = 32),
  CHECK (typeof(partition_bytes) = 'blob'
    AND length(partition_bytes) BETWEEN 1 AND 4096),
  CHECK (typeof(partition_hash) = 'blob' AND length(partition_hash) = 32),
  CHECK (active_generation > 0),
  CHECK (typeof(change_epoch) = 'text'
    AND length(change_epoch) BETWEEN 1 AND 128),
  CHECK (change_head >= 0),
  CHECK (compacted_through >= 0 AND compacted_through <= change_head),
  CHECK (pending_retry_failure_id IS NULL
    OR length(pending_retry_failure_id) BETWEEN 1 AND 255),
  CHECK (pending_retry_failure_id IS NULL OR stopped_failure_id IS NULL),
  CHECK (
    (stopped_failure_id IS NULL
      AND stopped_source_bytes IS NULL
      AND stopped_source_hash IS NULL
      AND stopped_source_partition_bytes IS NULL
      AND stopped_source_partition_hash IS NULL
      AND stopped_source_epoch IS NULL
      AND stopped_source_position IS NULL
      AND stopped_generation IS NULL
      AND stopped_input_hash IS NULL
      AND stopped_message_id IS NULL
      AND stopped_causation_id IS NULL
      AND stopped_gap_free IS NULL)
    OR
    (stopped_failure_id IS NOT NULL
      AND length(stopped_failure_id) BETWEEN 1 AND 255
      AND typeof(stopped_source_bytes) = 'blob'
      AND length(stopped_source_bytes) BETWEEN 1 AND 1024
      AND typeof(stopped_source_hash) = 'blob'
      AND length(stopped_source_hash) = 32
      AND typeof(stopped_source_partition_bytes) = 'blob'
      AND length(stopped_source_partition_bytes) BETWEEN 1 AND 4096
      AND typeof(stopped_source_partition_hash) = 'blob'
      AND length(stopped_source_partition_hash) = 32
      AND typeof(stopped_source_epoch) = 'text'
      AND length(stopped_source_epoch) BETWEEN 1 AND 128
      AND stopped_source_position IS NOT NULL
      AND stopped_source_position >= 0
      AND stopped_generation IS NOT NULL
      AND stopped_generation > 0
      AND typeof(stopped_input_hash) = 'blob'
      AND length(stopped_input_hash) = 32
      AND stopped_message_id IS NOT NULL
      AND length(stopped_message_id) BETWEEN 1 AND 255
      AND stopped_causation_id IS NOT NULL
      AND length(stopped_causation_id) BETWEEN 1 AND 128
      AND stopped_gap_free IS NOT NULL
      AND stopped_gap_free IN (0, 1))
  )
);

-- Repair generations are partition-wide durable facts. Generation one is the
-- original run; every later generation is linked to exactly one prior
-- generation and immutable terminal failure. The retry failure link is checked
-- by the repair transaction because a declarative foreign key would be
-- circular with projection_failures -> projection_generations.
CREATE TABLE IF NOT EXISTS projection_generations (
  topology_hash BLOB NOT NULL,
  partition_hash BLOB NOT NULL,
  generation INTEGER NOT NULL,
  retry_of_generation INTEGER,
  retry_of_failure_id TEXT,
  PRIMARY KEY (topology_hash, partition_hash, generation),
  UNIQUE (topology_hash, partition_hash, retry_of_failure_id),
  FOREIGN KEY (topology_hash, partition_hash)
    REFERENCES projection_partitions (topology_hash, partition_hash),
  CHECK (typeof(topology_hash) = 'blob' AND length(topology_hash) = 32),
  CHECK (typeof(partition_hash) = 'blob' AND length(partition_hash) = 32),
  CHECK (generation > 0),
  CHECK (
    (generation = 1
      AND retry_of_generation IS NULL
      AND retry_of_failure_id IS NULL)
    OR
    (generation > 1
      AND retry_of_generation IS NOT NULL
      AND retry_of_generation > 0
      AND retry_of_generation < generation
      AND retry_of_failure_id IS NOT NULL
      AND length(retry_of_failure_id) BETWEEN 1 AND 255)
  )
);

-- Durable ordering capability for one exact source scope. It is independent of
-- source epoch and repair generation, so a source cannot change between
-- gap-tolerant and gap-free ordering after either advancement or first failure.
CREATE TABLE IF NOT EXISTS projection_source_capabilities (
  topology_hash BLOB NOT NULL,
  partition_hash BLOB NOT NULL,
  source_bytes BLOB NOT NULL,
  source_hash BLOB NOT NULL,
  source_partition_bytes BLOB NOT NULL,
  source_partition_hash BLOB NOT NULL,
  gap_free INTEGER NOT NULL,
  PRIMARY KEY (
    topology_hash,
    partition_hash,
    source_hash,
    source_partition_hash
  ),
  FOREIGN KEY (topology_hash, partition_hash)
    REFERENCES projection_partitions (topology_hash, partition_hash),
  CHECK (typeof(topology_hash) = 'blob' AND length(topology_hash) = 32),
  CHECK (typeof(partition_hash) = 'blob' AND length(partition_hash) = 32),
  CHECK (typeof(source_bytes) = 'blob'
    AND length(source_bytes) BETWEEN 1 AND 1024),
  CHECK (typeof(source_hash) = 'blob' AND length(source_hash) = 32),
  CHECK (typeof(source_partition_bytes) = 'blob'
    AND length(source_partition_bytes) BETWEEN 1 AND 4096),
  CHECK (typeof(source_partition_hash) = 'blob'
    AND length(source_partition_hash) = 32),
  CHECK (gap_free IN (0, 1))
);

-- Generation-independent authenticated input identity. Repair generations
-- scope outcomes/re-execution, not the identity of a source cursor or message.
-- These rows survive repair and change-log compaction.
CREATE TABLE IF NOT EXISTS projection_input_identities (
  topology_hash BLOB NOT NULL,
  partition_hash BLOB NOT NULL,
  source_bytes BLOB NOT NULL,
  source_hash BLOB NOT NULL,
  source_partition_bytes BLOB NOT NULL,
  source_partition_hash BLOB NOT NULL,
  source_epoch TEXT NOT NULL,
  source_position INTEGER NOT NULL,
  input_hash BLOB NOT NULL,
  message_id TEXT NOT NULL,
  causation_id TEXT NOT NULL,
  gap_free INTEGER NOT NULL,
  PRIMARY KEY (
    topology_hash,
    partition_hash,
    source_hash,
    source_partition_hash,
    source_epoch,
    source_position
  ),
  UNIQUE (topology_hash, message_id),
  FOREIGN KEY (topology_hash, partition_hash)
    REFERENCES projection_partitions (topology_hash, partition_hash),
  CHECK (typeof(topology_hash) = 'blob' AND length(topology_hash) = 32),
  CHECK (typeof(partition_hash) = 'blob' AND length(partition_hash) = 32),
  CHECK (typeof(source_bytes) = 'blob'
    AND length(source_bytes) BETWEEN 1 AND 1024),
  CHECK (typeof(source_hash) = 'blob' AND length(source_hash) = 32),
  CHECK (typeof(source_partition_bytes) = 'blob'
    AND length(source_partition_bytes) BETWEEN 1 AND 4096),
  CHECK (typeof(source_partition_hash) = 'blob'
    AND length(source_partition_hash) = 32),
  CHECK (typeof(source_epoch) = 'text'
    AND length(source_epoch) BETWEEN 1 AND 128),
  CHECK (source_position >= 0),
  CHECK (typeof(input_hash) = 'blob' AND length(input_hash) = 32),
  CHECK (length(message_id) BETWEEN 1 AND 255),
  CHECK (length(causation_id) BETWEEN 1 AND 128),
  CHECK (gap_free IN (0, 1))
);

-- One durable last-good checkpoint per exact ordered source scope and repair
-- generation, including the change cursor allocated by its commit. Source
-- epoch is fenced row data: an epoch mismatch is incomparable and requires a
-- new projector topology plus read-model rebuild. Repair is failure-only and
-- cannot bridge source epochs.
CREATE TABLE IF NOT EXISTS projection_input_cursors (
  topology_hash BLOB NOT NULL,
  partition_hash BLOB NOT NULL,
  source_bytes BLOB NOT NULL,
  source_hash BLOB NOT NULL,
  source_partition_bytes BLOB NOT NULL,
  source_partition_hash BLOB NOT NULL,
  source_epoch TEXT NOT NULL,
  source_position INTEGER NOT NULL,
  input_hash BLOB NOT NULL,
  message_id TEXT NOT NULL,
  causation_id TEXT NOT NULL,
  gap_free INTEGER NOT NULL,
  generation INTEGER NOT NULL,
  change_epoch TEXT NOT NULL,
  change_position INTEGER NOT NULL,
  PRIMARY KEY (
    topology_hash,
    partition_hash,
    source_hash,
    source_partition_hash,
    generation
  ),
  UNIQUE (
    topology_hash,
    partition_hash,
    source_hash,
    source_partition_hash,
    message_id,
    generation
  ),
  FOREIGN KEY (topology_hash, partition_hash)
    REFERENCES projection_partitions (topology_hash, partition_hash),
  FOREIGN KEY (topology_hash, partition_hash, generation)
    REFERENCES projection_generations (
      topology_hash,
      partition_hash,
      generation
    ),
  CHECK (typeof(topology_hash) = 'blob' AND length(topology_hash) = 32),
  CHECK (typeof(partition_hash) = 'blob' AND length(partition_hash) = 32),
  CHECK (typeof(source_bytes) = 'blob'
    AND length(source_bytes) BETWEEN 1 AND 1024),
  CHECK (typeof(source_hash) = 'blob' AND length(source_hash) = 32),
  CHECK (typeof(source_partition_bytes) = 'blob'
    AND length(source_partition_bytes) BETWEEN 1 AND 4096),
  CHECK (typeof(source_partition_hash) = 'blob'
    AND length(source_partition_hash) = 32),
  CHECK (typeof(source_epoch) = 'text'
    AND length(source_epoch) BETWEEN 1 AND 128),
  CHECK (source_position >= 0),
  CHECK (typeof(input_hash) = 'blob' AND length(input_hash) = 32),
  CHECK (length(message_id) BETWEEN 1 AND 255),
  CHECK (length(causation_id) BETWEEN 1 AND 128),
  CHECK (gap_free IN (0, 1)),
  CHECK (generation > 0),
  CHECK (typeof(change_epoch) = 'text'
    AND length(change_epoch) BETWEEN 1 AND 128),
  CHECK (change_position > 0)
);

-- Append-only identity evidence for every terminal input outcome. Unlike the
-- mutable last-good cursor above, a receipt remains able to distinguish an exact
-- duplicate from message-ID or source-cursor reuse after later checkpoints
-- advance. Source scope is deliberately not part of the primary key so lookup
-- by partition generation and message ID also detects cross-source reuse.
-- The accepted change cursor remains durable after change-log compaction, so
-- it intentionally has no foreign key to projection_changes.
CREATE TABLE IF NOT EXISTS projection_input_receipts (
  topology_hash BLOB NOT NULL,
  partition_hash BLOB NOT NULL,
  generation INTEGER NOT NULL,
  message_id TEXT NOT NULL,
  source_bytes BLOB NOT NULL,
  source_hash BLOB NOT NULL,
  source_partition_bytes BLOB NOT NULL,
  source_partition_hash BLOB NOT NULL,
  source_epoch TEXT NOT NULL,
  source_position INTEGER NOT NULL,
  input_hash BLOB NOT NULL,
  causation_id TEXT NOT NULL,
  gap_free INTEGER NOT NULL,
  outcome_kind TEXT NOT NULL,
  failure_id TEXT,
  change_epoch TEXT NOT NULL,
  change_position INTEGER NOT NULL,
  PRIMARY KEY (
    topology_hash,
    partition_hash,
    generation,
    message_id
  ),
  UNIQUE (
    topology_hash,
    partition_hash,
    generation,
    source_hash,
    source_partition_hash,
    source_epoch,
    source_position
  ),
  UNIQUE (
    topology_hash,
    partition_hash,
    change_epoch,
    change_position
  ),
  FOREIGN KEY (topology_hash, partition_hash, generation)
    REFERENCES projection_generations (
      topology_hash,
      partition_hash,
      generation
    ),
  CHECK (typeof(topology_hash) = 'blob' AND length(topology_hash) = 32),
  CHECK (typeof(partition_hash) = 'blob' AND length(partition_hash) = 32),
  CHECK (generation > 0),
  CHECK (length(message_id) BETWEEN 1 AND 255),
  CHECK (typeof(source_bytes) = 'blob'
    AND length(source_bytes) BETWEEN 1 AND 1024),
  CHECK (typeof(source_hash) = 'blob' AND length(source_hash) = 32),
  CHECK (typeof(source_partition_bytes) = 'blob'
    AND length(source_partition_bytes) BETWEEN 1 AND 4096),
  CHECK (typeof(source_partition_hash) = 'blob'
    AND length(source_partition_hash) = 32),
  CHECK (typeof(source_epoch) = 'text'
    AND length(source_epoch) BETWEEN 1 AND 128),
  CHECK (source_position >= 0),
  CHECK (typeof(input_hash) = 'blob' AND length(input_hash) = 32),
  CHECK (length(causation_id) BETWEEN 1 AND 128),
  CHECK (gap_free IN (0, 1)),
  CHECK (outcome_kind IN ('applied', 'failed')),
  CHECK (
    (outcome_kind = 'applied' AND failure_id IS NULL)
    OR
    (outcome_kind = 'failed'
      AND failure_id IS NOT NULL
      AND length(failure_id) BETWEEN 1 AND 255)
  ),
  CHECK (typeof(change_epoch) = 'text'
    AND length(change_epoch) BETWEEN 1 AND 128),
  CHECK (change_position > 0)
);

-- Per-table serialization point shared by raw writers and causal bootstrap.
-- The first INSERT is a SQLite write and therefore serializes ownership
-- decisions until transaction commit, closing the absent-marker race.
CREATE TABLE IF NOT EXISTS projection_table_ownership_fences (
  table_name TEXT PRIMARY KEY,
  CHECK (length(table_name) BETWEEN 1 AND 255)
);

-- Model-wide bootstrap marker used by raw write paths, which do not carry a
-- typed projection partition. Presence makes the table exclusively causal;
-- partition-specific ownership below must reference this exact table/model
-- registration.
CREATE TABLE IF NOT EXISTS projection_causal_tables (
  table_name TEXT NOT NULL,
  model_name TEXT NOT NULL,
  ownership TEXT NOT NULL DEFAULT 'causal',
  PRIMARY KEY (table_name, model_name),
  UNIQUE (table_name),
  FOREIGN KEY (table_name)
    REFERENCES projection_table_ownership_fences (table_name),
  CHECK (length(table_name) BETWEEN 1 AND 255),
  CHECK (length(model_name) BETWEEN 1 AND 255),
  CHECK (ownership = 'causal')
);

-- Authoritative physical-table bootstrap declaration. Canonical topology bytes
-- make digest collisions detectable; global table uniqueness deliberately
-- forbids independent projector topologies from owning the same physical rows.
CREATE TABLE IF NOT EXISTS projection_registered_models (
  topology_bytes BLOB NOT NULL,
  topology_hash BLOB NOT NULL,
  model_name TEXT NOT NULL,
  table_name TEXT NOT NULL,
  PRIMARY KEY (topology_hash, model_name),
  UNIQUE (table_name),
  UNIQUE (topology_hash, table_name),
  UNIQUE (topology_hash, model_name, table_name),
  FOREIGN KEY (table_name, model_name)
    REFERENCES projection_causal_tables (table_name, model_name),
  CHECK (typeof(topology_bytes) = 'blob'
    AND length(topology_bytes) BETWEEN 1 AND 4096),
  CHECK (typeof(topology_hash) = 'blob' AND length(topology_hash) = 32),
  CHECK (length(model_name) BETWEEN 1 AND 255),
  CHECK (length(table_name) BETWEEN 1 AND 255)
);

-- Presence in this table is the fail-closed ownership marker. Raw/legacy write
-- plans must reject an exact model/partition scope found here.
CREATE TABLE IF NOT EXISTS projection_model_ownership (
  topology_hash BLOB NOT NULL,
  partition_hash BLOB NOT NULL,
  model_name TEXT NOT NULL,
  table_name TEXT NOT NULL,
  ownership TEXT NOT NULL DEFAULT 'causal',
  PRIMARY KEY (topology_hash, partition_hash, model_name),
  UNIQUE (topology_hash, partition_hash, table_name),
  FOREIGN KEY (topology_hash, partition_hash)
    REFERENCES projection_partitions (topology_hash, partition_hash),
  FOREIGN KEY (topology_hash, model_name, table_name)
    REFERENCES projection_registered_models (
      topology_hash,
      model_name,
      table_name
    ),
  CHECK (typeof(topology_hash) = 'blob' AND length(topology_hash) = 32),
  CHECK (typeof(partition_hash) = 'blob' AND length(partition_hash) = 32),
  CHECK (length(model_name) BETWEEN 1 AND 255),
  CHECK (length(table_name) BETWEEN 1 AND 255),
  CHECK (ownership = 'causal')
);

-- Revision comparison is valid only after exact topology, partition, model,
-- and canonical key scope equality. Tombstones remain durable; recreation
-- advances incarnation rather than resetting an old comparison domain.
CREATE TABLE IF NOT EXISTS projection_records (
  topology_hash BLOB NOT NULL,
  partition_hash BLOB NOT NULL,
  model_name TEXT NOT NULL,
  canonical_key_bytes BLOB NOT NULL,
  canonical_key_hash BLOB NOT NULL,
  incarnation INTEGER NOT NULL,
  revision INTEGER NOT NULL,
  tombstone INTEGER NOT NULL,
  change_epoch TEXT NOT NULL,
  change_position INTEGER NOT NULL,
  PRIMARY KEY (
    topology_hash,
    partition_hash,
    model_name,
    canonical_key_hash
  ),
  FOREIGN KEY (topology_hash, partition_hash, model_name)
    REFERENCES projection_model_ownership (
      topology_hash,
      partition_hash,
      model_name
    ),
  CHECK (typeof(topology_hash) = 'blob' AND length(topology_hash) = 32),
  CHECK (typeof(partition_hash) = 'blob' AND length(partition_hash) = 32),
  CHECK (length(model_name) BETWEEN 1 AND 255),
  CHECK (typeof(canonical_key_bytes) = 'blob'
    AND length(canonical_key_bytes) BETWEEN 1 AND 16384),
  CHECK (typeof(canonical_key_hash) = 'blob'
    AND length(canonical_key_hash) = 32),
  CHECK (incarnation > 0),
  CHECK (revision > 0),
  CHECK (tombstone IN (0, 1)),
  CHECK (typeof(change_epoch) = 'text'
    AND length(change_epoch) BETWEEN 1 AND 128),
  CHECK (change_position > 0)
);

-- A physical primary-key row can have only one live causal scope under its
-- owning topology. Tombstones may remain in prior partitions so an explicit
-- delete followed by a partition move preserves incarnation history.
CREATE UNIQUE INDEX IF NOT EXISTS projection_records_unique_live_identity
  ON projection_records (
    topology_hash,
    model_name,
    canonical_key_hash
  )
  WHERE tombstone = 0;

-- Exact causation evidence is joined by the same canonical record/dependency
-- scope used by command obligations; it is never inferred from a scalar max.
CREATE TABLE IF NOT EXISTS projection_observations (
  topology_hash BLOB NOT NULL,
  partition_hash BLOB NOT NULL,
  causation_id TEXT NOT NULL,
  model_name TEXT NOT NULL,
  scope_kind TEXT NOT NULL,
  canonical_key_bytes BLOB NOT NULL,
  canonical_key_hash BLOB NOT NULL,
  incarnation INTEGER,
  revision INTEGER,
  change_epoch TEXT NOT NULL,
  change_position INTEGER NOT NULL,
  PRIMARY KEY (
    topology_hash,
    partition_hash,
    causation_id,
    model_name,
    scope_kind,
    canonical_key_hash
  ),
  FOREIGN KEY (topology_hash, partition_hash, model_name)
    REFERENCES projection_model_ownership (
      topology_hash,
      partition_hash,
      model_name
    ),
  CHECK (typeof(topology_hash) = 'blob' AND length(topology_hash) = 32),
  CHECK (typeof(partition_hash) = 'blob' AND length(partition_hash) = 32),
  CHECK (length(causation_id) BETWEEN 1 AND 128),
  CHECK (length(model_name) BETWEEN 1 AND 255),
  CHECK (scope_kind IN ('record', 'dependency')),
  CHECK (typeof(canonical_key_bytes) = 'blob'
    AND length(canonical_key_bytes) BETWEEN 1 AND 16384),
  CHECK (typeof(canonical_key_hash) = 'blob'
    AND length(canonical_key_hash) = 32),
  CHECK (
    (scope_kind = 'record'
      AND incarnation IS NOT NULL
      AND incarnation > 0
      AND revision IS NOT NULL
      AND revision > 0)
    OR
    (scope_kind = 'dependency'
      AND incarnation IS NULL
      AND revision IS NULL)
  ),
  CHECK (typeof(change_epoch) = 'text'
    AND length(change_epoch) BETWEEN 1 AND 128),
  CHECK (change_position > 0)
);

-- An exact failed cursor/generation is an immutable terminal fact. Repair
-- creates a later linked generation; it never rewrites this row.
CREATE TABLE IF NOT EXISTS projection_failures (
  topology_hash BLOB NOT NULL,
  partition_hash BLOB NOT NULL,
  failure_id TEXT NOT NULL,
  source_bytes BLOB NOT NULL,
  source_hash BLOB NOT NULL,
  source_partition_bytes BLOB NOT NULL,
  source_partition_hash BLOB NOT NULL,
  source_epoch TEXT NOT NULL,
  source_position INTEGER NOT NULL,
  input_hash BLOB NOT NULL,
  message_id TEXT NOT NULL,
  causation_id TEXT NOT NULL,
  gap_free INTEGER NOT NULL,
  generation INTEGER NOT NULL,
  failure_code TEXT NOT NULL,
  failure_bytes BLOB NOT NULL,
  failure_hash BLOB NOT NULL,
  change_epoch TEXT NOT NULL,
  change_position INTEGER NOT NULL,
  PRIMARY KEY (
    topology_hash,
    partition_hash,
    source_hash,
    source_partition_hash,
    source_epoch,
    source_position,
    generation
  ),
  UNIQUE (failure_id),
  FOREIGN KEY (topology_hash, partition_hash)
    REFERENCES projection_partitions (topology_hash, partition_hash),
  FOREIGN KEY (topology_hash, partition_hash, generation)
    REFERENCES projection_generations (
      topology_hash,
      partition_hash,
      generation
    ),
  CHECK (typeof(topology_hash) = 'blob' AND length(topology_hash) = 32),
  CHECK (typeof(partition_hash) = 'blob' AND length(partition_hash) = 32),
  CHECK (length(failure_id) BETWEEN 1 AND 255),
  CHECK (typeof(source_bytes) = 'blob'
    AND length(source_bytes) BETWEEN 1 AND 1024),
  CHECK (typeof(source_hash) = 'blob' AND length(source_hash) = 32),
  CHECK (typeof(source_partition_bytes) = 'blob'
    AND length(source_partition_bytes) BETWEEN 1 AND 4096),
  CHECK (typeof(source_partition_hash) = 'blob'
    AND length(source_partition_hash) = 32),
  CHECK (typeof(source_epoch) = 'text'
    AND length(source_epoch) BETWEEN 1 AND 128),
  CHECK (source_position >= 0),
  CHECK (typeof(input_hash) = 'blob' AND length(input_hash) = 32),
  CHECK (length(message_id) BETWEEN 1 AND 255),
  CHECK (length(causation_id) BETWEEN 1 AND 128),
  CHECK (gap_free IN (0, 1)),
  CHECK (generation > 0),
  CHECK (length(failure_code) BETWEEN 1 AND 255),
  CHECK (typeof(failure_bytes) = 'blob'
    AND length(failure_bytes) BETWEEN 1 AND 1048576),
  CHECK (typeof(failure_hash) = 'blob' AND length(failure_hash) = 32),
  CHECK (typeof(change_epoch) = 'text'
    AND length(change_epoch) BETWEEN 1 AND 128),
  CHECK (change_position > 0)
);

-- Per-partition append log. Position zero is reserved for the empty
-- head/compacted-through watermark and is never a stored change.
CREATE TABLE IF NOT EXISTS projection_changes (
  topology_hash BLOB NOT NULL,
  partition_hash BLOB NOT NULL,
  change_epoch TEXT NOT NULL,
  change_position INTEGER NOT NULL,
  change_kind TEXT NOT NULL,
  causation_id TEXT NOT NULL,
  model_name TEXT,
  scope_kind TEXT,
  canonical_key_bytes BLOB,
  canonical_key_hash BLOB,
  incarnation INTEGER,
  revision INTEGER,
  failure_id TEXT,
  PRIMARY KEY (
    topology_hash,
    partition_hash,
    change_epoch,
    change_position
  ),
  FOREIGN KEY (topology_hash, partition_hash)
    REFERENCES projection_partitions (topology_hash, partition_hash),
  CHECK (typeof(topology_hash) = 'blob' AND length(topology_hash) = 32),
  CHECK (typeof(partition_hash) = 'blob' AND length(partition_hash) = 32),
  CHECK (typeof(change_epoch) = 'text'
    AND length(change_epoch) BETWEEN 1 AND 128),
  CHECK (change_position > 0),
  CHECK (change_kind IN (
    'record_upsert',
    'record_delete',
    'record_recreate',
    'observation',
    'checkpoint',
    'failure'
  )),
  CHECK (length(causation_id) BETWEEN 1 AND 128),
  CHECK (
    (change_kind IN ('record_upsert', 'record_delete', 'record_recreate')
      AND model_name IS NOT NULL
      AND length(model_name) BETWEEN 1 AND 255
      AND scope_kind IS NULL
      AND typeof(canonical_key_bytes) = 'blob'
      AND length(canonical_key_bytes) BETWEEN 1 AND 16384
      AND typeof(canonical_key_hash) = 'blob'
      AND length(canonical_key_hash) = 32
      AND incarnation IS NOT NULL
      AND incarnation > 0
      AND revision IS NOT NULL
      AND revision > 0
      AND failure_id IS NULL)
    OR
    (change_kind = 'observation'
      AND model_name IS NOT NULL
      AND length(model_name) BETWEEN 1 AND 255
      AND scope_kind IN ('record', 'dependency')
      AND typeof(canonical_key_bytes) = 'blob'
      AND length(canonical_key_bytes) BETWEEN 1 AND 16384
      AND typeof(canonical_key_hash) = 'blob'
      AND length(canonical_key_hash) = 32
      AND (
        (scope_kind = 'record'
          AND incarnation IS NOT NULL
          AND incarnation > 0
          AND revision IS NOT NULL
          AND revision > 0)
        OR
        (scope_kind = 'dependency'
          AND incarnation IS NULL
          AND revision IS NULL)
      )
      AND failure_id IS NULL)
    OR
    (change_kind = 'checkpoint'
      AND model_name IS NULL
      AND scope_kind IS NULL
      AND canonical_key_bytes IS NULL
      AND canonical_key_hash IS NULL
      AND incarnation IS NULL
      AND revision IS NULL
      AND failure_id IS NULL)
    OR
    (change_kind = 'failure'
      AND model_name IS NULL
      AND scope_kind IS NULL
      AND canonical_key_bytes IS NULL
      AND canonical_key_hash IS NULL
      AND incarnation IS NULL
      AND revision IS NULL
      AND failure_id IS NOT NULL
      AND length(failure_id) BETWEEN 1 AND 255)
  )
);

CREATE INDEX IF NOT EXISTS projection_records_change_idx
  ON projection_records (
    topology_hash,
    partition_hash,
    change_epoch,
    change_position
  );

CREATE INDEX IF NOT EXISTS projection_observations_causation_idx
  ON projection_observations (
    topology_hash,
    partition_hash,
    causation_id
  );

CREATE INDEX IF NOT EXISTS projection_input_receipts_causation_idx
  ON projection_input_receipts (
    topology_hash,
    partition_hash,
    causation_id
  );

CREATE INDEX IF NOT EXISTS projection_failures_causation_idx
  ON projection_failures (
    topology_hash,
    partition_hash,
    causation_id
  );

CREATE INDEX IF NOT EXISTS projection_changes_causation_idx
  ON projection_changes (
    topology_hash,
    partition_hash,
    causation_id
  );

CREATE INDEX IF NOT EXISTS projection_changes_record_idx
  ON projection_changes (
    topology_hash,
    partition_hash,
    model_name,
    canonical_key_hash,
    change_epoch,
    change_position
  );
