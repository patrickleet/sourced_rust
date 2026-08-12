/**
 * Framework host for pure reduces backed by a wasm-bindgen module.
 *
 * Domain pures validate record/args and return assign fields (or null).
 * This helper only: lazy-load, JSON bridge, fail-closed on any host error.
 */

import type { ReplicaValue } from '../types.js';
import type { ReplicaPureFunction } from './types.js';

/** Minimal wasm-bindgen module shape used by {@link createWasmJsonPure}. */
export type WasmJsonModule = {
	default: () => Promise<unknown>;
	[exportName: string]: unknown;
};

export type CreateWasmJsonPureOptions = Readonly<{
	/** Dynamic import of the wasm-pack module (e.g. `() => import('./pkg/x.js')`). */
	load: () => Promise<WasmJsonModule>;
	/**
	 * Named export that accepts `(recordJson: string, argsJson: string)` and
	 * returns a JSON object string of assign fields, or undefined/null to skip.
	 */
	exportName: string;
	/** When true (default in browsers), start loading immediately. */
	warm?: boolean;
}>;

export type WasmJsonPureHost = Readonly<{
	/** Resolve when the module is instantiated (no-op on non-browser). */
	ensureReady: () => Promise<void>;
	/** Sync pure for `pureFunctions` / pureReduces. */
	pure: ReplicaPureFunction;
}>;

function isBrowser(): boolean {
	return typeof globalThis !== 'undefined' && 'window' in globalThis;
}

/**
 * Build a {@link ReplicaPureFunction} that JSON-roundtrips through a WASM export.
 *
 * All field validation belongs in the WASM/domain pure. The host never inspects
 * individual record/arg keys — missing module or throw → null (fail closed).
 */
export function createWasmJsonPure(
	options: CreateWasmJsonPureOptions
): WasmJsonPureHost {
	let module: WasmJsonModule | null = null;
	let initPromise: Promise<void> | null = null;

	const ensureReady = (): Promise<void> => {
		if (!isBrowser()) {
			return Promise.resolve();
		}
		if (module) {
			return Promise.resolve();
		}
		if (initPromise) {
			return initPromise;
		}
		initPromise = options
			.load()
			.then(async (loaded) => {
				await loaded.default();
				module = loaded;
			})
			.catch((error) => {
				initPromise = null;
				module = null;
				throw error;
			});
		return initPromise;
	};

	if (options.warm !== false && isBrowser()) {
		void ensureReady().catch(() => {
			/* fail closed on first pure call */
		});
	}

	const pure: ReplicaPureFunction = (record, args) => {
		if (!module) {
			return null;
		}
		const fn = module[options.exportName];
		if (typeof fn !== 'function') {
			return null;
		}
		try {
			const out = (fn as (r: string, a: string) => string | undefined | null)(
				JSON.stringify(record),
				JSON.stringify(args)
			);
			if (out === undefined || out === null || out === '') {
				return null;
			}
			const parsed = JSON.parse(out) as unknown;
			if (parsed === null || typeof parsed !== 'object' || Array.isArray(parsed)) {
				return null;
			}
			return Object.freeze(parsed as Readonly<Record<string, ReplicaValue>>);
		} catch {
			return null;
		}
	};

	return Object.freeze({ ensureReady, pure });
}
