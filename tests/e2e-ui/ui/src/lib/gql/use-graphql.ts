/**
 * Browser binder: page data → GqlAuth + createGraphqlClient (POST /graphql).
 * Attaches generated command helpers as `client.commands.*`.
 */
import { bindCommands, type BoundCommands } from '$lib/api/commands.generated';
import { createGraphqlClient, type GraphqlClient } from './create-client.ts';
import { authFromPageData, type PageGraphqlData } from './auth-from-page.ts';

export type { PageGraphqlData } from './auth-from-page.ts';
export { authFromPageData } from './auth-from-page.ts';

/** Bound HTTP + WS client with pre-registered command functions. */
export type AppGraphqlClient = GraphqlClient & {
	commands: BoundCommands;
};

/**
 * Client bound to same-origin `/graphql` (Vite proxies to the API in dev).
 * Pass a getter so reactive page data is read on each request / subscribe.
 *
 * @example
 * const gql = useGraphql(() => data);
 * await gql.commands.todosCreate({ todo_id, title });
 * gql.subscribe(chat.subscription, { onNext });
 */
export function useGraphql(getData: () => PageGraphqlData): AppGraphqlClient {
	const client = createGraphqlClient({
		getUrl: () => '/graphql',
		getAuth: () => authFromPageData(getData())
	});
	return {
		...client,
		commands: bindCommands(client)
	};
}
