/**
 * Client pure for `blob.simulate_move` — thin host over `blob-domain` WASM.
 *
 * Board rules live once in Rust (`blob_domain::core::simulate_move`). This
 * module only loads the wasm package, adapts replica record/args, and fails
 * closed when the module is not ready or the move is impossible.
 *
 * Build: `make wasm` (or `make ui-install`) → `./pkg` from wasm-pack
 * (`--features wasm --no-default-features` on blob-domain).
 */

export type BlobMoveResult = Readonly<{
	map_json: string;
	score: number;
	player_dead: boolean;
	current_level_completed: boolean;
	status: string;
}>;

type WasmApi = {
	default: (input?: unknown) => Promise<unknown>;
	blobSimulateMove: (
		map_json: string,
		score: number,
		direction: string
	) => string | undefined;
};

let api: WasmApi | null = null;
let initPromise: Promise<void> | null = null;

function isBrowser(): boolean {
	return typeof window !== 'undefined';
}

/**
 * Load and instantiate blob-domain pure WASM. Safe to call multiple times.
 * No-op on the server (SSR) — pure reduce fails closed until the client inits.
 */
export function ensureBlobWasm(): Promise<void> {
	if (!isBrowser()) {
		return Promise.resolve();
	}
	if (api) {
		return Promise.resolve();
	}
	if (initPromise) {
		return initPromise;
	}
	initPromise = (async () => {
		const mod = (await import('./pkg/blob_wasm.js')) as WasmApi;
		await mod.default();
		api = mod;
	})().catch((error) => {
		initPromise = null;
		api = null;
		throw error;
	});
	return initPromise;
}

// Warm the module early on the client so the first move can pure-reduce.
if (isBrowser()) {
	void ensureBlobWasm().catch(() => {
		// Fail closed later in simulateMove; do not break module load.
	});
}

/**
 * Apply one direction to a known BlobGames row (WASM pure).
 * Returns null when not ready, invalid, or the move is impossible.
 */
export function simulateMove(
	record: Readonly<Record<string, unknown>>,
	args: Readonly<Record<string, unknown>>
): BlobMoveResult | null {
	if (!api) {
		return null;
	}
	const direction = args.direction;
	if (typeof direction !== 'string') return null;
	const mapJson = record.map_json;
	if (typeof mapJson !== 'string') return null;
	const scoreRaw = record.score;
	const score =
		typeof scoreRaw === 'number'
			? scoreRaw
			: typeof scoreRaw === 'bigint'
				? Number(scoreRaw)
				: typeof scoreRaw === 'string'
					? Number(scoreRaw)
					: NaN;
	if (!Number.isFinite(score)) return null;

	let json: string | undefined;
	try {
		json = api.blobSimulateMove(mapJson, score, direction);
	} catch {
		return null;
	}
	if (json === undefined || json === '') return null;
	try {
		const parsed = JSON.parse(json) as BlobMoveResult;
		if (
			typeof parsed.map_json !== 'string' ||
			typeof parsed.score !== 'number' ||
			typeof parsed.player_dead !== 'boolean' ||
			typeof parsed.current_level_completed !== 'boolean' ||
			typeof parsed.status !== 'string'
		) {
			return null;
		}
		return Object.freeze(parsed);
	} catch {
		return null;
	}
}

/** Pure registry entry for the command runtime (and generated pures.ts). */
export const BLOB_PURE_FUNCTIONS = Object.freeze({
	'blob.simulate_move': simulateMove
});
