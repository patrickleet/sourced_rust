/** Framework-neutral reactive document store backed by {@link QueryCache}. */
import type { GraphqlClient } from './client.js';
import type { CacheTarget, ListMergeSpec } from './cache/ops.js';
import { writeServerDataPreservingPending } from './cache/ops.js';
import { cacheKey, type QueryCache } from './cache/query-cache.js';
import { documentToString, type GqlDocument } from './document.js';
import type { GraphqlVariables } from './types.js';

export type StoreStatus = 'idle' | 'connecting' | 'live' | 'error';

export type DocumentStoreSnapshot<TSelected = unknown> = {
	/** Selected view of the cached document. */
	data: TSelected;
	/** Full cached payload before selection. */
	raw: unknown;
	status: StoreStatus;
	error: string | null;
	pending: boolean;
	optimistic: boolean;
};

export type DocumentStoreOptions<
	TData = Record<string, unknown>,
	TSelected = TData
> = {
	/** Query or subscription document. */
	document: GqlDocument;
	variables?: GraphqlVariables;
	/** Server-rendered or first-paint data used to seed the cache. */
	initialData?: TData;
	/** Map cached document data to the application-facing view. */
	select?: (data: TData) => TSelected;
	/** Open a live GraphQL subscription when true. */
	live?: boolean;
	/** Required to preserve pending optimistic rows in list documents. */
	list?: ListMergeSpec;
};

export type DocumentStore<TSelected = unknown> = {
	/** Svelte-compatible readable-store contract; usable by any subscriber. */
	subscribe: (
		run: (value: DocumentStoreSnapshot<TSelected>) => void
	) => () => void;
	get: () => DocumentStoreSnapshot<TSelected>;
	readonly document: string;
	readonly variables: GraphqlVariables | undefined;
	readonly list: ListMergeSpec | undefined;
	/** Build a cache target for command policies and optimistic updates. */
	target: (at: string, by: string) => CacheTarget;
	seed: (data: unknown) => void;
	refetch: () => Promise<void>;
	scheduleCatchUp: (delayMs?: number) => void;
	connect: () => void;
	destroy: () => void;
};

function identity<T>(value: T): T {
	return value;
}

/** Bind one GraphQL document to a client's shared cache. */
export function createDocumentStore<
	TData = Record<string, unknown>,
	TSelected = TData
>(
	client: GraphqlClient,
	options: DocumentStoreOptions<TData, TSelected>
): DocumentStore<TSelected> {
	if (!client.cache) {
		throw new Error('createDocumentStore requires a GraphqlClient with a cache');
	}
	const queryCache: QueryCache = client.cache;
	const document = documentToString(options.document);
	const variables = options.variables;
	const list = options.list;
	const key = cacheKey(document, variables);
	const select = (options.select ?? identity) as (data: TData) => TSelected;
	const listeners = new Set<(value: DocumentStoreSnapshot<TSelected>) => void>();

	let status: StoreStatus = options.live ? 'connecting' : 'idle';
	let error: string | null = null;
	let unsubscribeCache: (() => void) | null = null;
	let unsubscribeWebSocket: (() => void) | null = null;
	let catchUpTimer: ReturnType<typeof setTimeout> | null = null;
	let destroyed = false;

	function snapshot(): DocumentStoreSnapshot<TSelected> {
		const entry = queryCache.get(key);
		const raw = (entry?.data ?? options.initialData ?? null) as TData;
		let data: TSelected;
		try {
			data = select(raw);
		} catch {
			data = select((options.initialData ?? {}) as TData);
		}

		return {
			data,
			raw: entry?.data,
			status,
			error,
			pending: entry?.pending === true,
			optimistic: entry?.optimistic === true
		};
	}

	function emit(): void {
		const value = snapshot();
		for (const listener of listeners) listener(value);
	}

	function seed(data: unknown): void {
		if (destroyed) return;
		const existing = queryCache.get(key);
		if (existing?.optimistic || existing?.pending) {
			emit();
			return;
		}

		const merged = list ? mergeSeedLists(data, existing?.data, list) : undefined;
		queryCache.set(key, {
			data: merged ?? data,
			updatedAt: Date.now(),
			optimistic: false,
			pending: false
		});
	}

	if (options.initialData !== undefined) seed(options.initialData);
	unsubscribeCache = queryCache.subscribe(key, emit);

	function connect(): void {
		if (destroyed || !options.live) return;
		unsubscribeWebSocket?.();
		status = 'connecting';
		error = null;
		emit();

		unsubscribeWebSocket = client.subscribe(
			options.document,
			{
				onNext: (payload) => {
					if (payload.errors?.length) {
						status = 'error';
						error = payload.errors[0]?.message ?? 'subscription error';
						emit();
						return;
					}
					if (payload.data !== undefined && payload.data !== null) {
						status = 'live';
						error = null;
						// A package client owns guarded write-through. Structural custom clients
						// retain the small fallback for compatibility.
						if (
							client.writesToCache !== true &&
							queryCache.get(key)?.data === undefined
						) {
							queryCache.set(key, {
								data: payload.data,
								updatedAt: Date.now(),
								optimistic: false,
								pending: false
							});
						} else {
							emit();
						}
					}
				},
				onError: (cause) => {
					status = 'error';
					error = websocketErrorMessage(cause);
					emit();
				},
				onComplete: () => {
					if (status === 'live') {
						status = 'connecting';
						emit();
					}
				}
			},
			{ variables, list }
		);
	}

	if (options.live && typeof window !== 'undefined') {
		queueMicrotask(() => {
			if (!destroyed) connect();
		});
	}

	async function refetch(): Promise<void> {
		if (destroyed) return;
		const cacheGeneration = queryCache.generation;
		const result = await client.request<TData>(
			options.document,
			variables,
			list ? { list } : undefined
		);
		if (destroyed || queryCache.generation !== cacheGeneration) return;

		if (result.errors?.length) {
			error = result.errors[0]?.message ?? 'refetch failed';
			emit();
			return;
		}
		if (
			client.writesToCache !== true &&
			result.data !== undefined &&
			result.data !== null
		) {
			writeServerDataPreservingPending(
				queryCache,
				document,
				variables,
				result.data,
				list ? { list } : undefined
			);
		}
		error = null;
		emit();
	}

	function scheduleCatchUp(delayMs = 800): void {
		if (destroyed) return;
		if (catchUpTimer !== null) clearTimeout(catchUpTimer);
		catchUpTimer = setTimeout(() => {
			catchUpTimer = null;
			if (destroyed) return;
			void refetch().catch((cause: unknown) => {
				if (destroyed) return;
				error = cause instanceof Error ? cause.message : String(cause);
				emit();
			});
		}, delayMs);
	}

	return {
		subscribe(run) {
			listeners.add(run);
			run(snapshot());
			return () => listeners.delete(run);
		},
		get: snapshot,
		document,
		variables,
		list,
		target(at, by) {
			return { document, variables, at, by };
		},
		seed,
		refetch,
		scheduleCatchUp,
		connect,
		destroy() {
			if (destroyed) return;
			destroyed = true;
			if (catchUpTimer !== null) clearTimeout(catchUpTimer);
			catchUpTimer = null;
			unsubscribeCache?.();
			unsubscribeCache = null;
			unsubscribeWebSocket?.();
			unsubscribeWebSocket = null;
			listeners.clear();
		}
	};
}

function mergeSeedLists(
	serverData: unknown,
	localData: unknown,
	list: ListMergeSpec
): unknown | undefined {
	if (
		serverData === null ||
		typeof serverData !== 'object' ||
		localData === null ||
		typeof localData !== 'object'
	) {
		return undefined;
	}

	const parts = list.at.split('.').filter(Boolean);
	const serverList = listAt(serverData, parts);
	const localList = listAt(localData, parts);
	if (!serverList || !localList) return undefined;

	const byId = new Map<unknown, Record<string, unknown>>();
	for (const row of [...serverList, ...localList]) {
		if (row !== null && typeof row === 'object') {
			const record = row as Record<string, unknown>;
			byId.set(record[list.by], record);
		}
	}
	const mergedList = [...byId.values()];
	if (parts.length === 0) return mergedList;

	const root = { ...(serverData as Record<string, unknown>) };
	let current = root;
	for (let index = 0; index < parts.length - 1; index += 1) {
		const part = parts[index]!;
		const next = current[part];
		current[part] =
			next !== null && typeof next === 'object' && !Array.isArray(next)
				? { ...(next as Record<string, unknown>) }
				: {};
		current = current[part] as Record<string, unknown>;
	}
	current[parts[parts.length - 1]!] = mergedList;
	return root;
}

function listAt(document: unknown, parts: string[]): unknown[] | null {
	let current = document;
	for (const part of parts) {
		if (current === null || typeof current !== 'object') return null;
		current = (current as Record<string, unknown>)[part];
	}
	return Array.isArray(current) ? current : null;
}

function websocketErrorMessage(error: unknown): string {
	if (typeof Event !== 'undefined' && error instanceof Event) {
		return 'WebSocket error — is the API running?';
	}
	return Array.isArray(error) ? JSON.stringify(error) : String(error);
}
