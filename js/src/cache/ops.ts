/** Closed cache-operation and UI-effect sets for command processing. */
import type { GraphqlVariables } from '../types.js';
import {
	cacheKey,
	type CacheEntry,
	type CacheKey,
	type QueryCache
} from './query-cache.js';

export type CacheTarget = {
	/** Query or subscription source string. */
	document: string;
	variables?: GraphqlVariables;
	/** Dot path to a list/object. Empty or omitted means the whole document. */
	at?: string;
	/** Primary-key field used for list upserts and removals. */
	by?: string;
};

export type CacheOp =
	| { op: 'upsert'; target: CacheTarget; row: Record<string, unknown> }
	| { op: 'patch'; target: CacheTarget; row: Record<string, unknown> }
	| { op: 'remove'; target: CacheTarget; id: unknown }
	| { op: 'write'; target: CacheTarget; data: unknown }
	| {
			op: 'invalidate';
			/** Exact cache key. Omit to invalidate the complete cache. */
			prefix?: string;
	  };

export type Effect =
	| { kind: 'toast'; message: string }
	| { kind: 'alert'; message: string }
	| { kind: 'goto'; path: string };

/** UI side-effect constructors that do not collide with Svelte's `$effect` rune. */
export const fx = {
	toast: (message: string): Effect => ({ kind: 'toast', message }),
	alert: (message: string): Effect => ({ kind: 'alert', message }),
	goto: (path: string): Effect => ({ kind: 'goto', path })
};

export type ResultKind = 'ack' | 'fact' | 'projection' | 'none';
export type ReconcileKind = 'subscription' | 'refetch' | 'invalidate' | 'none';

export type ListMergeSpec = {
	/** Dot path to the list, for example `todos`. */
	at: string;
	/** Primary-key field on each row, for example `todo_id`. */
	by: string;
};

export type CommandPolicy = {
	result?: { kind: ResultKind; apply?: { targets: CacheTarget[] } };
	reconcile?: {
		kind: ReconcileKind;
		document?: string;
		variables?: GraphqlVariables;
		list?: ListMergeSpec;
	};
	optimistic?: {
		targets: CacheTarget[];
		row?: Record<string, unknown>;
	};
};

export type Snapshot = {
	key: CacheKey;
	entry: CacheEntry<unknown> | undefined;
	had: boolean;
};

function getAt(data: unknown, path?: string): unknown {
	if (!path) return data;
	let current = data;
	for (const part of path.split('.').filter(Boolean)) {
		if (current === null || typeof current !== 'object') return undefined;
		current = (current as Record<string, unknown>)[part];
	}
	return current;
}

function setAt(data: unknown, path: string | undefined, value: unknown): unknown {
	if (!path) return value;
	const parts = path.split('.').filter(Boolean);
	if (parts.length === 0) return value;

	const root =
		data !== null && typeof data === 'object' ? { ...(data as Record<string, unknown>) } : {};
	let current: Record<string, unknown> = root;
	for (let index = 0; index < parts.length - 1; index += 1) {
		const part = parts[index]!;
		const next = current[part];
		current[part] =
			next !== null && typeof next === 'object' && !Array.isArray(next)
				? { ...(next as Record<string, unknown>) }
				: {};
		current = current[part] as Record<string, unknown>;
	}
	current[parts[parts.length - 1]!] = value;
	return root;
}

function cloneEntry(entry: CacheEntry<unknown> | undefined): CacheEntry<unknown> | undefined {
	return entry ? { ...entry, data: structuredClone(entry.data) } : undefined;
}

/** Apply one operation and return the pre-operation snapshot for rollback. */
export function applyCacheOp(
	cache: QueryCache,
	operation: CacheOp,
	now = Date.now()
): Snapshot | null {
	if (operation.op === 'invalidate') {
		cache.invalidate(operation.prefix);
		return null;
	}

	const key = cacheKey(operation.target.document, operation.target.variables);
	const previous = cache.get(key);
	const snapshot: Snapshot = { key, entry: cloneEntry(previous), had: previous !== undefined };

	if (operation.op === 'write') {
		cache.set(key, {
			data: operation.data,
			updatedAt: now,
			optimistic: false,
			pending: false
		});
		return snapshot;
	}

	const baseData = previous?.data ?? {};
	const at = operation.target.at;
	const by = operation.target.by ?? 'id';

	if (operation.op === 'upsert' || operation.op === 'patch') {
		const list = getAt(baseData, at);
		if (Array.isArray(list)) {
			const id = operation.row[by];
			const existingIndex = list.findIndex(
				(row) =>
					row !== null &&
					typeof row === 'object' &&
					(row as Record<string, unknown>)[by] === id
			);
			const next = [...list];
			if (existingIndex >= 0) {
				next[existingIndex] = {
					...(next[existingIndex] as Record<string, unknown>),
					...operation.row
				};
			} else if (operation.op === 'upsert') {
				next.push({ ...operation.row });
			}

			cache.set(key, {
				data: setAt(baseData, at, next),
				updatedAt: now,
				optimistic: true,
				pending: previous?.pending
			});
		} else if (!previous) {
			if (!at) {
				cache.set(key, { data: operation.row, updatedAt: now, optimistic: true });
			}
		} else if (!at && operation.op === 'patch' && typeof previous.data === 'object') {
			cache.set(key, {
				data: { ...(previous.data as object), ...operation.row },
				updatedAt: now,
				optimistic: true
			});
		}
		return snapshot;
	}

	const list = getAt(baseData, at);
	if (Array.isArray(list)) {
		const next = list.filter(
			(row) =>
				!(
					row !== null &&
					typeof row === 'object' &&
					(row as Record<string, unknown>)[by] === operation.id
				)
		);
		cache.set(key, {
			data: setAt(baseData, at, next),
			updatedAt: now,
			optimistic: false
		});
	}
	return snapshot;
}

/** Apply operations in order and return rollback snapshots. */
export function applyCacheOps(cache: QueryCache, operations: CacheOp[]): Snapshot[] {
	const snapshots: Snapshot[] = [];
	for (const operation of operations) {
		const snapshot = applyCacheOp(cache, operation);
		if (snapshot) snapshots.push(snapshot);
	}
	return snapshots;
}

/** Restore snapshots in reverse order. */
export function rollback(cache: QueryCache, snapshots: Snapshot[]): void {
	for (let index = snapshots.length - 1; index >= 0; index -= 1) {
		const snapshot = snapshots[index]!;
		if (snapshot.had && snapshot.entry) cache.set(snapshot.key, snapshot.entry);
		else cache.delete(snapshot.key);
	}
}

/** Merge a projection payload's present fields onto each cache target. */
export function applyProjectionPayload(
	cache: QueryCache,
	targets: CacheTarget[],
	payload: Record<string, unknown>
): Snapshot[] {
	return applyCacheOps(
		cache,
		targets.map((target) => ({ op: 'upsert', target, row: { ...payload } }))
	);
}

export type WriteServerOptions = {
	/** Required to merge list documents while optimistic rows are pending. */
	list?: ListMergeSpec;
};

/**
 * Write server data without clobbering optimistic rows that an asynchronous
 * projector has not exposed yet.
 */
export function writeServerDataPreservingPending(
	cache: QueryCache,
	document: string,
	variables: GraphqlVariables | undefined,
	serverData: unknown,
	options?: WriteServerOptions
): void {
	const key = cacheKey(document, variables);
	const previous = cache.get(key);
	const now = Date.now();

	if (!previous?.pending && !previous?.optimistic) {
		cache.set(key, {
			data: serverData,
			updatedAt: now,
			pending: false,
			optimistic: false
		});
		return;
	}

	if (
		previous.data === null ||
		typeof previous.data !== 'object' ||
		serverData === null ||
		typeof serverData !== 'object'
	) {
		return;
	}

	const list = options?.list;
	if (!list?.at || !list.by) return;

	const serverList = getAt(serverData, list.at);
	const localList = getAt(previous.data, list.at);
	if (!Array.isArray(serverList) || !Array.isArray(localList)) return;

	const serverById = rowsById(serverList, list.by);
	const localById = rowsById(localList, list.by);
	const merged: Record<string, unknown>[] = [];
	const seen = new Set<unknown>();

	for (const row of serverList) {
		if (row === null || typeof row !== 'object') continue;
		const serverRow = row as Record<string, unknown>;
		const id = serverRow[list.by];
		seen.add(id);
		const localRow = localById.get(id);
		merged.push(localRow && shallowRowDiffers(localRow, serverRow) ? { ...localRow } : { ...serverRow });
	}

	for (const [id, localRow] of localById) {
		if (!seen.has(id) && !serverById.has(id)) merged.push({ ...localRow });
	}

	const stillPending = merged.some((row) => {
		const serverRow = serverById.get(row[list.by]);
		return !serverRow || shallowRowDiffers(row, serverRow);
	});

	cache.set(key, {
		data: setAt(serverData, list.at, merged),
		updatedAt: now,
		pending: stillPending,
		optimistic: stillPending
	});
}

function rowsById(rows: unknown[], by: string): Map<unknown, Record<string, unknown>> {
	const result = new Map<unknown, Record<string, unknown>>();
	for (const row of rows) {
		if (row !== null && typeof row === 'object') {
			const record = row as Record<string, unknown>;
			result.set(record[by], record);
		}
	}
	return result;
}

function shallowRowDiffers(
	left: Record<string, unknown>,
	right: Record<string, unknown>
): boolean {
	for (const key of new Set([...Object.keys(left), ...Object.keys(right)])) {
		if (left[key] !== right[key]) return true;
	}
	return false;
}
