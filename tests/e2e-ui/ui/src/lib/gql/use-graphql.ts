/**
 * Browser binder: page data → GqlAuth + createGraphqlClient (POST /graphql).
 *
 * Cache is **transparent** (Houdini-style):
 * - `gql.store` / `gql.live` seed + follow the QueryCache automatically
 * - `request` / `subscribe` write-through into the cache
 * - `gql.commands.*` pipeline patches the same cache keys
 *
 * Pages should not call manual cache seed helpers by hand — use gql.store / gql.live.
 */
import { createGraphqlClient, type GraphqlClient } from './create-client.ts';
import { authFromPageData, type PageGraphqlData } from './auth-from-page.ts';
import {
	bindCommandsPipeline,
	type CommandPolicyMap,
	type PipelinedBoundCommands
} from './bind-commands-pipeline.ts';
import { QueryCache } from './cache/query-cache.ts';
import type { Effect } from './cache/ops.ts';
import {
	createDocumentStore,
	type DocumentStore,
	type DocumentStoreOptions
} from './document-store.ts';
import { e2eCommandPolicies } from './command-policies.ts';

export type { PageGraphqlData } from './auth-from-page.ts';
export { authFromPageData } from './auth-from-page.ts';
export { e2eCommandPolicies } from './command-policies.ts';

/** Bound HTTP + WS client with pipelined commands + document stores. */
export type AppGraphqlClient = GraphqlClient & {
	commands: PipelinedBoundCommands;
	/** Shared browser query cache (escape hatch; prefer store/live). */
	cache: QueryCache;
	/**
	 * Follow a query document in the cache (SSR seed + refetch/optimistic updates).
	 * Use `$store.data` in templates.
	 */
	store: <TData = Record<string, unknown>, TSelected = TData>(
		options: DocumentStoreOptions<TData, TSelected>
	) => DocumentStore<TSelected>;
	/**
	 * Like `store` + automatic GraphQL subscription for the same document.
	 * Connection status is on `$store.status`.
	 */
	live: <TData = Record<string, unknown>, TSelected = TData>(
		options: Omit<DocumentStoreOptions<TData, TSelected>, 'live'>
	) => DocumentStore<TSelected>;
};

export type UseGraphqlOptions = {
	/** Override the shared browser cache (default: new QueryCache). */
	cache?: QueryCache;
	/** Per-command default result/reconcile policies. */
	policies?: CommandPolicyMap;
	/** Optional UI effect handler (toast/alert). */
	runEffects?: (effects: Effect[]) => void;
};

/**
 * Identity key for C-U18 cache isolation.
 * Bearer: JWT `payload.sub` when parseable (RS256 JWTs share a header prefix — do **not**
 * key on token.slice(0, N)). Non-JWT tokens: hash of the full token string.
 * DevHeaders: userId + role.
 */
export function authIdentityKey(auth: import('./types.ts').GqlAuth): string {
	const token = auth.accessToken?.trim() || '';
	if (token) {
		const sub = jwtPayloadSub(token);
		if (sub) return `sub:${sub}`;
		return `bearer:${hashString(token)}`;
	}
	return `dev:${auth.userId ?? ''}:${auth.role ?? ''}`;
}

/** Decode JWT payload `sub` (middle segment). Null if not a 3-part JWT or no sub. */
export function jwtPayloadSub(token: string): string | null {
	const parts = token.split('.');
	if (parts.length !== 3 || !parts[1]) return null;
	try {
		const json = base64UrlDecode(parts[1]);
		const payload = JSON.parse(json) as { sub?: unknown };
		return typeof payload.sub === 'string' && payload.sub.length > 0 ? payload.sub : null;
	} catch {
		return null;
	}
}

function base64UrlDecode(segment: string): string {
	const pad = segment.length % 4 === 0 ? '' : '='.repeat(4 - (segment.length % 4));
	const b64 = segment.replace(/-/g, '+').replace(/_/g, '/') + pad;
	if (typeof atob === 'function') {
		return atob(b64);
	}
	// Node unit tests
	return Buffer.from(b64, 'base64').toString('utf8');
}

/** FNV-1a 32-bit — enough to distinguish opaque bearer strings without leaking full token in keys. */
function hashString(s: string): string {
	let h = 0x811c9dc5;
	for (let i = 0; i < s.length; i++) {
		h ^= s.charCodeAt(i);
		h = Math.imul(h, 0x01000193);
	}
	return (h >>> 0).toString(16);
}

/**
 * Client bound to same-origin `/graphql`.
 *
 * @example Chat (cache transparent)
 * const gql = useGraphql(() => data);
 * const lobby = gql.live({
 *   document: chat.subscription ?? chat.query,
 *   initialData: { chat_messages: data.messages },
 *   select: (d) => d.chat_messages ?? [],
 * });
 * // {$lobby.data} {$lobby.status}
 * onDestroy(() => lobby.destroy());
 *
 * @example Command with optimistic list patch (policies default fact + none)
 * await gql.commands.todosCreate(input, {
 *   optimistic: { targets: [list.target('todos', 'todo_id')], row },
 * });
 * list.scheduleCatchUp(); // soft delayed refetch after projector lag
 */
export function useGraphql(
	getData: () => PageGraphqlData,
	options: UseGraphqlOptions = {}
): AppGraphqlClient {
	// Prefer a fresh QueryCache per binder so identity switches (new page load /
	// remount after login) do not reuse another principal's document keys (C-U18).
	// Callers may still pass a shared `options.cache` for tests; clear it on auth id change.
	const cache = options.cache ?? new QueryCache();
	let lastAuthId = authIdentityKey(authFromPageData(getData()));
	const getAuth = () => {
		const auth = authFromPageData(getData());
		const id = authIdentityKey(auth);
		if (id !== lastAuthId) {
			cache.clear();
			lastAuthId = id;
		}
		return auth;
	};
	const client = createGraphqlClient({
		getUrl: () => '/graphql',
		getAuth,
		cache,
		writeThrough: true
	});

	function store<TData = Record<string, unknown>, TSelected = TData>(
		storeOpts: DocumentStoreOptions<TData, TSelected>
	): DocumentStore<TSelected> {
		return createDocumentStore(client, storeOpts);
	}

	function live<TData = Record<string, unknown>, TSelected = TData>(
		storeOpts: Omit<DocumentStoreOptions<TData, TSelected>, 'live'>
	): DocumentStore<TSelected> {
		return createDocumentStore(client, { ...storeOpts, live: true });
	}

	return {
		...client,
		cache,
		store,
		live,
		// GraphqlClient.request is a structural match for CommandClient.
		commands: bindCommandsPipeline(client as import('$lib/api/commands.generated').CommandClient, {
			cache,
			policies: options.policies ?? e2eCommandPolicies,
			runEffects: options.runEffects
		})
	};
}
