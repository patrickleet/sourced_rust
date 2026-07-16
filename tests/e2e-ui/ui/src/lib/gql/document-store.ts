/**
 * Houdini-inspired document store: the cache is transparent.
 *
 * App code does **not** seed/subscribe/sync the QueryCache by hand.
 * A store:
 *   1. Seeds the shared cache from SSR / initial data
 *   2. Subscribes to that cache key (optimistic + refetch + live writes update UI)
 *   3. Optionally opens a GraphQL subscription (write-through already in createClient)
 *
 * Usage (Svelte — `$` auto-subscribes to the store contract):
 *
 *   const lobby = gql.live({
 *     document: chat.subscription ?? chat.query,
 *     initialData: { chat_messages: data.messages },
 *     select: (d) => sortChatMessages(d.chat_messages ?? []),
 *   });
 *   // {$lobby.data}  {$lobby.status}
 *
 *   await gql.commands.chatMessagesPost(input, {
 *     result: { kind: 'fact' },
 *     reconcile: { kind: 'subscription', document: lobby.document },
 *     optimistic: { targets: [lobby.target('chat_messages', 'message_id')], row },
 *   });
 */

import type { GqlDocument } from './document.ts';
import { documentToString } from './document.ts';
import type { QueryCache } from './cache/query-cache.ts';
import { cacheKey } from './cache/query-cache.ts';
import type { CacheTarget } from './cache/ops.ts';
import type { GraphqlClient } from './create-client.ts';

export type StoreStatus = 'idle' | 'connecting' | 'live' | 'error';

export type DocumentStoreSnapshot<TSelected = unknown> = {
	/** Selected view of the cached document data. */
	data: TSelected;
	/** Full document payload in cache (before select). */
	raw: unknown;
	status: StoreStatus;
	error: string | null;
	pending: boolean;
	optimistic: boolean;
};

export type DocumentStoreOptions<TData = Record<string, unknown>, TSelected = TData> = {
	/** Query or subscription document (same selection set preferred). */
	document: GqlDocument;
	variables?: Record<string, unknown>;
	/** SSR / first-paint payload written into the cache. */
	initialData?: TData;
	/** Map cached document → UI shape (e.g. list extract + sort). */
	select?: (data: TData) => TSelected;
	/**
	 * When true, open a live GraphQL subscription for `document`.
	 * Cache write-through is automatic; UI follows the cache.
	 */
	live?: boolean;
};

export type DocumentStore<TSelected = unknown> = {
	/** Svelte store contract — use as `{$store.data}`. */
	subscribe: (run: (value: DocumentStoreSnapshot<TSelected>) => void) => () => void;
	/** Current snapshot (non-reactive outside Svelte). */
	get: () => DocumentStoreSnapshot<TSelected>;
	/** GraphQL source string (for command reconcile / policies). */
	readonly document: string;
	readonly variables: Record<string, unknown> | undefined;
	/** Cache target for optimistic list upserts. */
	target: (at: string, by: string) => CacheTarget;
	/** Replace SSR seed / external server data without dropping optimistic if newer. */
	seed: (data: unknown) => void;
	/** Force HTTP refetch of the same document (write-through updates cache → UI). */
	refetch: () => Promise<void>;
	/** (Re)connect live subscription. */
	connect: () => void;
	/** Tear down cache listener + WS. Call from onDestroy. */
	destroy: () => void;
};

function identity<T>(x: T): T {
	return x;
}

/**
 * Create a document-keyed store bound to a client + its QueryCache.
 * Prefer `gql.store` / `gql.live` from `useGraphql`.
 */
export function createDocumentStore<TData = Record<string, unknown>, TSelected = TData>(
	client: GraphqlClient,
	options: DocumentStoreOptions<TData, TSelected>
): DocumentStore<TSelected> {
	const cache = client.cache;
	if (!cache) {
		throw new Error('createDocumentStore requires a GraphqlClient with cache');
	}

	const docStr = documentToString(options.document);
	const variables = options.variables;
	const key = cacheKey(docStr, variables);
	const select = (options.select ?? identity) as (data: TData) => TSelected;

	const listeners = new Set<(v: DocumentStoreSnapshot<TSelected>) => void>();
	let status: StoreStatus = options.live ? 'connecting' : 'idle';
	let error: string | null = null;
	let unsubCache: (() => void) | null = null;
	let unsubWs: (() => void) | null = null;
	let destroyed = false;

	function readRaw(): unknown {
		return cache.get(key)?.data;
	}

	function snapshot(): DocumentStoreSnapshot<TSelected> {
		const entry = cache.get(key);
		const raw = (entry?.data ?? options.initialData ?? null) as TData;
		let data: TSelected;
		try {
			data = select(raw as TData);
		} catch {
			data = select((options.initialData ?? {}) as TData);
		}
		return {
			data,
			raw: entry?.data,
			status,
			error,
			pending: !!entry?.pending,
			optimistic: !!entry?.optimistic
		};
	}

	function emit() {
		const snap = snapshot();
		for (const run of listeners) run(snap);
	}

	function seed(data: unknown) {
		if (destroyed) return;
		// Don't clobber a fresher optimistic/pending entry with older SSR.
		const existing = cache.get(key);
		if (existing?.optimistic || existing?.pending) {
			emit();
			return;
		}
		cache.set(key, {
			data,
			updatedAt: Date.now(),
			optimistic: false,
			pending: false
		});
	}

	// Initial seed
	if (options.initialData !== undefined) {
		seed(options.initialData);
	}

	unsubCache = cache.subscribe(key, () => emit());

	function connect() {
		if (destroyed || !options.live) return;
		unsubWs?.();
		status = 'connecting';
		error = null;
		emit();
		unsubWs = client.subscribe(options.document, {
			onNext: (payload) => {
				const p = payload as { data?: unknown; errors?: Array<{ message?: string }> };
				if (p?.errors?.length) {
					status = 'error';
					error = p.errors[0]?.message ?? 'subscription error';
					emit();
					return;
				}
				// createClient already write-throughs data into cache; just mark live.
				if (p?.data !== undefined) {
					status = 'live';
					error = null;
					// Ensure write-through even if client was built without cache (defensive)
					if (!cache.get(key)?.data && p.data) {
						cache.set(key, {
							data: p.data,
							updatedAt: Date.now(),
							optimistic: false,
							pending: false
						});
					} else {
						emit();
					}
				}
			},
			onError: (e) => {
				status = 'error';
				if (e instanceof Event) {
					error = 'WebSocket error — is the API running?';
				} else if (Array.isArray(e)) {
					error = JSON.stringify(e);
				} else {
					error = String(e);
				}
				emit();
			},
			onComplete: () => {
				if (status === 'live') {
					status = 'connecting';
					emit();
				}
			}
		});
	}

	if (options.live && typeof window !== 'undefined') {
		// Defer to next microtask so callers can attach subscribe first.
		queueMicrotask(() => {
			if (!destroyed) connect();
		});
	}

	async function refetch() {
		const result = await client.request(options.document, variables as never);
		if (result.errors?.length) {
			error = result.errors[0]?.message ?? 'refetch failed';
			emit();
			return;
		}
		// Prefer client write-through; still write here so plain mock clients work.
		if (result.data !== undefined && result.data !== null) {
			cache.set(key, {
				data: result.data,
				updatedAt: Date.now(),
				optimistic: false,
				pending: false
			});
		}
		error = null;
		emit();
	}

	return {
		subscribe(run) {
			listeners.add(run);
			run(snapshot());
			return () => {
				listeners.delete(run);
			};
		},
		get: snapshot,
		document: docStr,
		variables,
		target(at: string, by: string): CacheTarget {
			return { document: docStr, variables, at, by };
		},
		seed,
		refetch,
		connect,
		destroy() {
			destroyed = true;
			unsubCache?.();
			unsubCache = null;
			unsubWs?.();
			unsubWs = null;
			listeners.clear();
		}
	};
}
