import {
	createGraphqlClient,
	type GraphqlClient,
	type GraphqlClientOptions
} from '../client.js';
import type {
	BindCommandsOptions,
	BoundCommandSet,
	CommandClient,
	CommandDefinitionMap,
	CommandPolicyMap
} from '../commands.js';
import { authIdentityKey } from '../identity.js';
import { QueryCache } from '../cache/query-cache.js';
import type { Effect } from '../cache/ops.js';
import {
	createDocumentStore,
	type DocumentStore,
	type DocumentStoreOptions
} from '../document-store.js';
import type { GqlAuth } from '../types.js';
import { authFromPageData, type PageGraphqlData } from './auth.js';

type EmptyCommands = Readonly<Record<never, never>>;

export type CommandBinder<TDefinitions extends CommandDefinitionMap> = (
	client: CommandClient,
	options?: BindCommandsOptions<TDefinitions>
) => BoundCommandSet<TDefinitions>;

/** GraphQL client plus transparent document stores and generated command functions. */
export type SveltekitGraphqlClient<
	TDefinitions extends CommandDefinitionMap = EmptyCommands
> = GraphqlClient & {
	commands: BoundCommandSet<TDefinitions>;
	cache: QueryCache;
	store: <TData = Record<string, unknown>, TSelected = TData>(
		options: DocumentStoreOptions<TData, TSelected>
	) => DocumentStore<TSelected>;
	live: <TData = Record<string, unknown>, TSelected = TData>(
		options: Omit<DocumentStoreOptions<TData, TSelected>, 'live'>
	) => DocumentStore<TSelected>;
};

export type CreateUseGraphqlOptions<
	TDefinitions extends CommandDefinitionMap = EmptyCommands
> = {
	/** Generated binder from `distributed-gen-commands`. Omit for read-only apps. */
	bindCommands?: CommandBinder<TDefinitions>;
	/** Generated service policies; individual calls may override them. */
	policies?: CommandPolicyMap<TDefinitions>;
	/** Browser endpoint. Defaults to the same-origin `/graphql` proxy. */
	url?: string | (() => string);
	/** Override page-data auth mapping for a custom session shape. */
	getAuth?: (data: PageGraphqlData) => GqlAuth | Promise<GqlAuth>;
};

export type UseGraphqlOptions<TDefinitions extends CommandDefinitionMap = EmptyCommands> = {
	/** Override the fresh, per-binder cache (primarily useful in tests). */
	cache?: QueryCache;
	policies?: CommandPolicyMap<TDefinitions>;
	runEffects?: (effects: Effect[]) => void;
	/** Advanced client overrides without replacing the package composition. */
	client?: Pick<GraphqlClientOptions, 'fetch' | 'webSocket'>;
};

/**
 * Configure an app once with its generated command module, then bind each page:
 *
 * `export const useGraphql = createUseGraphql({ bindCommands, policies })`
 */
export function createUseGraphql<
	TDefinitions extends CommandDefinitionMap = EmptyCommands
>(defaults: CreateUseGraphqlOptions<TDefinitions> = {}) {
	return function useGraphql(
		getData: () => PageGraphqlData,
		options: UseGraphqlOptions<TDefinitions> = {}
	): SveltekitGraphqlClient<TDefinitions> {
		const cache = options.cache ?? new QueryCache();
		const mapAuth = defaults.getAuth ?? authFromPageData;
		// Capture the initial principal at bind time. Transitions are serialized in
		// invocation order so a slow old mapper cannot overwrite a newer identity.
		let currentAuthId: string | undefined;
		const initialData = getData();
		let identityQueue = Promise.resolve()
			.then(() => mapAuth(initialData))
			.then(
				(auth) => {
					currentAuthId = authIdentityKey(auth);
				},
				() => {
					// If the baseline cannot be established, retain no private cache.
					cache.clear();
				}
			);

		const getAuth = () => {
			const pageData = getData();
			const authPromise = Promise.resolve().then(() => mapAuth(pageData));
			const transition = identityQueue.then(async () => {
				let auth: GqlAuth;
				try {
					auth = await authPromise;
				} catch (error) {
					cache.clear();
					currentAuthId = undefined;
					throw error;
				}
				const id = authIdentityKey(auth);
				if (currentAuthId !== undefined && id !== currentAuthId) cache.clear();
				currentAuthId = id;
				return auth;
			});
			// A failed mapping rejects this request without poisoning later transitions.
			identityQueue = transition.then(
				() => undefined,
				() => undefined
			);
			return transition;
		};

		const configuredUrl = defaults.url;
		const getUrl =
			typeof configuredUrl === 'function'
				? configuredUrl
				: () => configuredUrl ?? '/graphql';
		const client = createGraphqlClient({
			getUrl,
			getAuth,
			cache,
			writeThrough: true,
			...options.client
		});

		const store = <TData = Record<string, unknown>, TSelected = TData>(
			storeOptions: DocumentStoreOptions<TData, TSelected>
		) => createDocumentStore(client, storeOptions);
		const live = <TData = Record<string, unknown>, TSelected = TData>(
			storeOptions: Omit<DocumentStoreOptions<TData, TSelected>, 'live'>
		) => createDocumentStore(client, { ...storeOptions, live: true });

		const bindOptions: BindCommandsOptions<TDefinitions> = {
			cache,
			policies: options.policies ?? defaults.policies,
			runEffects: options.runEffects
		};
		const commands = defaults.bindCommands
			? defaults.bindCommands(client, bindOptions)
			: ({} as BoundCommandSet<TDefinitions>);

		return { ...client, cache, store, live, commands };
	};
}

/** Convenience type for a read-only adapter with no generated commands. */
export type ReadonlySveltekitGraphqlClient = SveltekitGraphqlClient<EmptyCommands>;
