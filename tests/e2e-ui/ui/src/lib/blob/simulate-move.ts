/**
 * Client pure for `blob.simulate_move` — framework WASM host + domain pure.
 *
 * Rules + validation live in `blob_domain::core` (exported as WASM).
 * This file only wires the module path for gen-client / pureFunctions.
 *
 * Build: `make wasm` → `./pkg` (`blob-domain --features wasm`).
 */

import { createWasmJsonPure } from '@hops-ops/distributed/replica';

const host = createWasmJsonPure({
	load: () => import('./pkg/blob_wasm.js'),
	exportName: 'blobSimulateMove'
});

/** Load WASM before first move so pure-reduce can paint (SSR is a no-op). */
export const ensureBlobWasm = host.ensureReady;

/** Replica pure: known BlobGames row + `{ direction }` → assign fields | null. */
export const simulateMove = host.pure;

/** Pure registry entry for the command runtime (and generated pures.ts). */
export const BLOB_PURE_FUNCTIONS = Object.freeze({
	'blob.simulate_move': simulateMove
});
