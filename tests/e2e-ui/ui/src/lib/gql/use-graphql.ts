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
	const cache = options.cache ?? new QueryCache();
	const client = createGraphqlClient({
		getUrl: () => '/graphql',
		getAuth: () => authFromPageData(getData()),
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
