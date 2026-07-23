/** Unified HTTP and WebSocket GraphQL client with optional cache write-through. */
import { Kind, parse, type DocumentNode } from 'graphql';
import type { TypedDocumentNode } from '@graphql-typed-document-node/core';

import {
	writeServerDataPreservingPending,
	type ListMergeSpec
} from './cache/ops.js';
import type { QueryCache } from './cache/query-cache.js';
import { documentToString, type GqlDocument } from './document.js';
import { authIdentityKey } from './identity.js';
import type { DistributedLiveCursor } from './protocol.js';
import {
	requestGraphql,
	type FetchLike
} from './request.js';
import type {
	GqlAuth,
	GqlResult,
	GraphqlVariables
} from './types.js';
import {
	subscribe as subscribeWs,
	type GqlWsHandlers,
	type WebSocketConstructor
} from './websocket.js';

export type GraphqlClientOptions = {
	/** Absolute API URL or same-origin path such as `/graphql`. */
	getUrl: () => string;
	getAuth: () => GqlAuth | Promise<GqlAuth>;
	/** Query/subscription cache. */
	cache?: QueryCache;
	/** Defaults to true when a cache is supplied. */
	writeThrough?: boolean;
	/** Override the runtime's global fetch implementation. */
	fetch?: FetchLike;
	/** Override the runtime's global WebSocket constructor. */
	webSocket?: WebSocketConstructor;
};

export type RequestWriteOptions = {
	/** Merge spec used to retain pending optimistic rows. */
	list?: ListMergeSpec;
	/** Framework status/recovery operations bypass application query caching. */
	cache?: 'write' | 'skip';
};

export type SubscribeCallOptions<
	TVariables extends GraphqlVariables = GraphqlVariables
> = {
	variables?: TVariables;
	/** Merge spec used to retain pending optimistic rows. */
	list?: ListMergeSpec;
	/**
	 * Server-issued cursors retained by the generated live coordinator.
	 * @internal Application code must not synthesize resume tokens.
	 */
	resume?: readonly DistributedLiveCursor[];
};

export type GraphqlClient = {
	request: <
		TData = Record<string, unknown>,
		TVariables extends GraphqlVariables = GraphqlVariables
	>(
		document: GqlDocument<TData, TVariables>,
		variables?: TVariables,
		writeOptions?: RequestWriteOptions
	) => Promise<GqlResult<TData>>;
	subscribe: <
		TData = unknown,
		TVariables extends GraphqlVariables = GraphqlVariables
	>(
		document: GqlDocument<TData, TVariables>,
		handlers: GqlWsHandlers<TData>,
		options?: SubscribeCallOptions<TVariables>
	) => () => void;
	cache?: QueryCache;
	/** True when this client owns successful query/subscription cache writes. */
	writesToCache?: boolean;
};

/**
 * Return true when any executable operation in `document` is a mutation.
 * Parsing handles comments and insignificant whitespace before the operation.
 */
export function looksLikeMutation(
	document: GqlDocument | TypedDocumentNode<unknown, GraphqlVariables>
): boolean {
	let node: DocumentNode;
	try {
		node = typeof document === 'string' ? parse(document) : document;
	} catch {
		// Invalid documents cannot produce successful data, so they are safe to leave
		// to the server's normal validation/error path.
		return false;
	}

	return node.definitions.some(
		(definition) =>
			definition.kind === Kind.OPERATION_DEFINITION && definition.operation === 'mutation'
	);
}

/** Bind URL, authentication, transports, and optional cache into one client. */
export function createGraphqlClient(options: GraphqlClientOptions): GraphqlClient {
	const cache = options.cache;
	const writeThrough = options.writeThrough ?? options.cache !== undefined;
	const getAuthFailClosed = async (): Promise<GqlAuth> => {
		const generation = cache?.generation;
		try {
			return await options.getAuth();
		} catch (error) {
			if (cache && cache.generation === generation) cache.clear();
			throw error;
		}
	};

	const authStillOwnsCache = async (
		initialAuth: GqlAuth,
		generation: number | undefined
	): Promise<boolean> => {
		const currentAuth = await getAuthFailClosed();

		const sameIdentity = authIdentityKey(currentAuth) === authIdentityKey(initialAuth);
		if (!sameIdentity && cache && cache.generation === generation) {
			cache.clear();
		}

		return (
			sameIdentity &&
			(!cache || cache.generation === generation)
		);
	};

	return {
		cache: options.cache,
		writesToCache: options.cache !== undefined && writeThrough,
		async request<
			TData = Record<string, unknown>,
			TVariables extends GraphqlVariables = GraphqlVariables
		>(
			document: GqlDocument<TData, TVariables>,
			variables?: TVariables,
			writeOptions?: RequestWriteOptions
		): Promise<GqlResult<TData>> {
			const auth = await getAuthFailClosed();
			const cacheGeneration = options.cache?.generation;
			const result = await requestGraphql(
				options.getUrl(),
				document,
				auth,
				variables ?? ({} as TVariables),
				options.fetch ? { fetch: options.fetch } : {}
			);
			const cacheOwnershipCurrent = options.cache
				? await authStillOwnsCache(auth, cacheGeneration)
				: true;

			if (
				options.cache &&
				writeThrough &&
				writeOptions?.cache !== 'skip' &&
				cacheOwnershipCurrent &&
				result.data !== undefined &&
				result.data !== null &&
				!result.errors?.length &&
				!looksLikeMutation(document)
			) {
				writeServerDataPreservingPending(
					options.cache,
					documentToString(document),
					variables,
					result.data,
					writeOptions?.list ? { list: writeOptions.list } : undefined
				);
			}

			return result;
		},
		subscribe<
			TData = unknown,
			TVariables extends GraphqlVariables = GraphqlVariables
		>(
			document: GqlDocument<TData, TVariables>,
			handlers: GqlWsHandlers<TData>,
			callOptions: SubscribeCallOptions<TVariables> = {}
		): () => void {
			let unsubscribe = (): void => {};
			let cancelled = false;
			let callbackQueue = Promise.resolve();
			const variables = callOptions.variables ?? ({} as TVariables);
			const list = callOptions.list;
			const cancel = () => {
				if (cancelled) return;
				cancelled = true;
				unsubscribe();
			};

			void (async () => {
				try {
					const auth = await getAuthFailClosed();
					if (cancelled) return;
					const cacheGeneration = options.cache?.generation;
					const enqueue = (callback: () => void) => {
						callbackQueue = callbackQueue
							.then(async () => {
								if (cancelled) return;
								if (!(await authStillOwnsCache(auth, cacheGeneration))) {
									cancel();
									return;
								}
								callback();
							})
							.catch((error: unknown) => {
								if (!cancelled) handlers.onError?.(error);
								cancel();
							});
					};
					const wrappedHandlers: GqlWsHandlers<TData> = {
						onNext: (payload) => {
							enqueue(() => {
								if (
									options.cache &&
									writeThrough &&
									payload.data !== undefined &&
									payload.data !== null &&
									!payload.errors?.length
								) {
									writeServerDataPreservingPending(
										options.cache,
										documentToString(document),
										variables,
										payload.data,
										list ? { list } : undefined
									);
								}
								handlers.onNext(payload);
							});
						},
						onError: (error) => {
							enqueue(() => handlers.onError?.(error));
						},
						onComplete: () => {
							enqueue(() => handlers.onComplete?.());
						}
					};
					unsubscribe = subscribeWs(document, auth, wrappedHandlers, {
						httpUrl: options.getUrl(),
						variables,
						resume: callOptions.resume,
						webSocket: options.webSocket
					});
					if (cancelled) unsubscribe();
				} catch (error) {
					if (!cancelled) handlers.onError?.(error);
				}
			})();

			return () => {
				cancel();
			};
		}
	};
}
