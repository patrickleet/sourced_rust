/**
 * Thin unified GraphQL client factory.
 * Inject URL + auth so SSR private env never enters the isomorphic core.
 * Documents may be strings or TypedDocumentNode from `.gql` codegen.
 */
import type { TypedDocumentNode } from '@graphql-typed-document-node/core';
import { requestGraphql, type GqlDocument } from './request.ts';
import type { GqlAuth, GqlResult } from './types.ts';

export type GraphqlClientOptions = {
	/** Absolute API URL or same-origin path, e.g. `/graphql` or `http://127.0.0.1:8791/graphql` */
	getUrl: () => string;
	getAuth: () => GqlAuth | Promise<GqlAuth>;
};

export type GraphqlClient = {
	request: <
		TResult = Record<string, unknown>,
		TVariables extends Record<string, unknown> = Record<string, unknown>
	>(
		document: GqlDocument | TypedDocumentNode<TResult, TVariables>,
		variables?: TVariables
	) => Promise<GqlResult<TResult>>;
};

export function createGraphqlClient(opts: GraphqlClientOptions): GraphqlClient {
	return {
		async request<
			TResult = Record<string, unknown>,
			TVariables extends Record<string, unknown> = Record<string, unknown>
		>(
			document: GqlDocument | TypedDocumentNode<TResult, TVariables>,
			variables?: TVariables
		): Promise<GqlResult<TResult>> {
			const auth = await opts.getAuth();
			return requestGraphql<TResult, TVariables>(
				opts.getUrl(),
				document,
				auth,
				(variables ?? {}) as TVariables
			);
		}
	};
}
