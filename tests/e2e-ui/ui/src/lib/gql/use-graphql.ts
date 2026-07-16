/**
 * Browser binder: page data → GqlAuth + createGraphqlClient (POST /graphql).
 * Attaches generated command helpers as `client.commands.*` through the
 * command result pipeline when a browser QueryCache is available.
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

export type { PageGraphqlData } from './auth-from-page.ts';
export { authFromPageData } from './auth-from-page.ts';

/** Bound HTTP + WS client with pipelined command functions + shared cache. */
export type AppGraphqlClient = GraphqlClient & {
	commands: PipelinedBoundCommands;
	cache: QueryCache;
};

export type UseGraphqlOptions = {
	/** Override / seed the shared browser cache (default: new QueryCache). */
	cache?: QueryCache;
	/** Per-command default result/reconcile policies. */
	policies?: CommandPolicyMap;
	/** Optional UI effect handler (toast/alert). */
	runEffects?: (effects: Effect[]) => void;
};

/**
 * Client bound to same-origin `/graphql` (Vite proxies to the API in dev).
 * Pass a getter so reactive page data is read on each request / subscribe.
 *
 * @example
 * const gql = useGraphql(() => data);
 * await gql.commands.todosCreate(
 *   { todo_id, title },
 *   { optimistic: { targets: [...], row }, result: { kind: 'fact' }, reconcile: { kind: 'refetch', document } }
 * );
 * gql.subscribe(chat.subscription, { onNext });
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
	return {
		...client,
		cache,
		commands: bindCommandsPipeline(client, {
			cache,
			policies: options.policies,
			runEffects: options.runEffects
		})
	};
}
