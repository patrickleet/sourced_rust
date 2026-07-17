/**
 * Thin unified GraphQL client factory.
 * Inject URL + auth so SSR private env never enters the isomorphic core.
 * Documents may be strings or TypedDocumentNode from `.gql` codegen.
 *
 * Browser clients also expose `subscribe` so WebSocket auth/URL match HTTP —
 * no separate `authFromPageData` at call sites.
 *
 * Optional `cache`: query/subscription write-through (never treats mutations as
 * projected list truth — command pipeline owns command result apply).
 */
import type { TypedDocumentNode } from '@graphql-typed-document-node/core';
import {
	subscribe as subscribeWs,
	type GqlWsHandlers
} from '../graphql-ws.ts';
import { requestGraphql, type GqlDocument } from './request.ts';
import { documentToString } from './document.ts';
import type { GqlAuth, GqlResult } from './types.ts';
import type { QueryCache } from './cache/query-cache.ts';
import {
	writeServerDataPreservingPending,
	type ListMergeSpec
} from './cache/ops.ts';

export type GraphqlClientOptions = {
	/** Absolute API URL or same-origin path, e.g. `/graphql` or `http://127.0.0.1:8791/graphql` */
	getUrl: () => string;
	getAuth: () => GqlAuth | Promise<GqlAuth>;
	/** Browser query/subscription cache (optional). */
	cache?: QueryCache;
	/** When true (default if cache set), write query results into the cache. */
	writeThrough?: boolean;
};

export type SubscribeCallOptions = {
	/** Must match document-store / request cache key variables. */
	variables?: Record<string, unknown>;
	/** List merge for pending optimistic rows on write-through. */
	list?: ListMergeSpec;
};

export type GraphqlClient = {
	request: <
		TResult = Record<string, unknown>,
		TVariables extends Record<string, unknown> = Record<string, unknown>
	>(
		document: GqlDocument | TypedDocumentNode<TResult, TVariables>,
		variables?: TVariables,
		/** Optional list merge for pending write-through. */
		writeOpts?: { list?: ListMergeSpec }
	) => Promise<GqlResult<TResult>>;
	/**
	 * Live subscription over `/graphql/ws` using the same auth as `request`.
	 * Returns an unsubscribe function (safe to call before the socket opens).
	 * When a cache is configured, successful payloads write-through by document+variables key.
	 */
	subscribe: (
		document: GqlDocument,
		handlers: GqlWsHandlers,
		options?: SubscribeCallOptions
	) => () => void;
	/** Shared query cache when configured on the client. */
	cache?: QueryCache;
};

/** Exported for unit tests — mutation responses must not write-through as query data. */
export function looksLikeMutation(
	document: GqlDocument | TypedDocumentNode<unknown, unknown>
): boolean {
	const src = documentToString(document as GqlDocument).trimStart();
	return /^mutation[\s({]/i.test(src);
}

export function createGraphqlClient(opts: GraphqlClientOptions): GraphqlClient {
	const writeThrough = opts.writeThrough ?? !!opts.cache;

	const client: GraphqlClient = {
		cache: opts.cache,
		async request<
			TResult = Record<string, unknown>,
			TVariables extends Record<string, unknown> = Record<string, unknown>
		>(
			document: GqlDocument | TypedDocumentNode<TResult, TVariables>,
			variables?: TVariables,
			writeOpts?: { list?: ListMergeSpec }
		): Promise<GqlResult<TResult>> {
			const auth = await opts.getAuth();
			const result = await requestGraphql<TResult, TVariables>(
				opts.getUrl(),
				document as GqlDocument,
				auth,
				(variables ?? {}) as TVariables
			);
			// Query write-through only — mutations go through the command pipeline.
			// Preserve pending optimistic rows (async projector lag).
			if (
				opts.cache &&
				writeThrough &&
				result.data &&
				!result.errors?.length &&
				!looksLikeMutation(document)
			) {
				writeServerDataPreservingPending(
					opts.cache,
					documentToString(document as GqlDocument),
					(variables ?? {}) as Record<string, unknown>,
					result.data,
					writeOpts?.list ? { list: writeOpts.list } : undefined
				);
			}
			return result;
		},
		subscribe(document, handlers, callOpts = {}) {
			let unsub = () => {};
			let cancelled = false;
			const variables = callOpts.variables ?? {};
			const list = callOpts.list;
			const wrapped: GqlWsHandlers = {
				...handlers,
				onNext: (payload) => {
					if (opts.cache && writeThrough && payload && typeof payload === 'object') {
						const p = payload as {
							data?: unknown;
							errors?: unknown[];
						};
						if (p.data && !p.errors?.length) {
							writeServerDataPreservingPending(
								opts.cache,
								documentToString(document),
								variables,
								p.data,
								list ? { list } : undefined
							);
						}
					}
					handlers.onNext?.(payload);
				}
			};
			void (async () => {
				const auth = await opts.getAuth();
				if (cancelled) return;
				unsub = subscribeWs(document, auth, wrapped, {
					httpUrl: opts.getUrl(),
					variables
				});
			})();
			return () => {
				cancelled = true;
				unsub();
			};
		}
	};
	return client;
}
