/**
 * Unit tests for browser command pipeline + QueryCache.
 * Drives the published `@hops-ops/distributed/cache` entry point.
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import path from 'node:path';

const uiRoot = path.dirname(fileURLToPath(new URL('.', import.meta.url)));

// Keep the detailed TAP-style pack in one helper while exercising only public
// package exports. `--experimental-strip-types` also loads app-owned generated TS.

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

test('systems-harden unit suite (strip-types)', () => {
  const harden = path.join(uiRoot, 'tests/systems-harden-unit.mjs');
  const hr = spawnSync(process.execPath, ['--experimental-strip-types', harden], {
    encoding: 'utf8',
    cwd: uiRoot
  });
  assert.equal(hr.status, 0, `systems-harden unit failed:\n${hr.stdout}\n${hr.stderr}`);
  assert.match(hr.stdout, /# pass|# tests|ok -/);
});
