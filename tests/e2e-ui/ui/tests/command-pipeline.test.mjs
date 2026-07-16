/**
 * Unit tests for browser command pipeline + QueryCache.
 * Drives the shipped modules under src/lib/gql/cache/.
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { pathToFileURL } from 'node:url';
import path from 'node:path';
import { createRequire } from 'node:module';

const uiRoot = path.dirname(fileURLToPath(new URL('.', import.meta.url)));
const cacheDir = path.join(uiRoot, '../src/lib/gql/cache');

// Node cannot import .ts directly — load compiled-equivalent via dynamic import
// of the TypeScript sources through a small ESM bridge using tsx if present,
// otherwise re-implement is forbidden: use node --experimental-strip-types when available.

import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';

const runner = path.join(uiRoot, 'tests/run-pipeline-unit.mjs');
const r = spawnSync(process.execPath, ['--experimental-strip-types', runner], {
  encoding: 'utf8',
  cwd: uiRoot
});

if (r.status !== 0) {
  // Fallback: inline minimal loader that imports .ts with strip-types from this file
  console.error(r.stdout);
  console.error(r.stderr);
}

test('pipeline unit suite (strip-types)', () => {
  if (r.status === 0) {
    assert.match(r.stdout + r.stderr, /# pass|# tests|ok |passed/i);
    return;
  }
  // If strip-types unsupported, fail with diagnostics
  assert.equal(
    r.status,
    0,
    `pipeline unit runner failed:\n${r.stdout}\n${r.stderr}`
  );
});
