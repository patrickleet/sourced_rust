/**
 * Single HTTP GraphQL request path for browser and SSR.
 * URL and auth are injected by callers (see createGraphqlClient / wrappers).
 * Accepts string or TypedDocumentNode (from co-located .gql codegen).
 */
import type { TypedDocumentNode } from '@graphql-typed-document-node/core';
import { buildAuthHeaders } from './auth-headers.ts';
import { documentToString, type GqlDocument } from './document.ts';
import type { GqlAuth, GqlResult } from './types.ts';

export type { GqlDocument } from './document.ts';
export { documentToString } from './document.ts';
export { buildAuthHeaders } from './auth-headers.ts';

/**
 * POST GraphQL document to `url`. Real entry point used by browser + SSR wrappers.
 */
export async function requestGraphql<
	TResult = Record<string, unknown>,
	TVariables extends Record<string, unknown> = Record<string, unknown>
>(
	url: string,
	document: GqlDocument | TypedDocumentNode<TResult, TVariables>,
	auth: GqlAuth = {},
	variables: TVariables | Record<string, unknown> = {}
): Promise<GqlResult<TResult>> {
	const token = auth.accessToken?.trim() || '';
	const query = documentToString(document as GqlDocument);
	const res = await fetch(url, {
		method: 'POST',
		headers: buildAuthHeaders(auth),
		body: JSON.stringify({ query, variables })
	});

	const body = (await res.json().catch(() => ({}))) as {
		data?: TResult;
		errors?: Array<{ message: string }>;
		error?: string;
	};

	if (res.status === 401) {
		const detail =
			body.errors?.[0]?.message ||
			body.error ||
			(token
				? 'Bearer rejected (audience/issuer/expiry) — sign out and back in'
				: 'no access token — re-login; check OIDC_SCOPES includes project aud');
		return {
			data: body.data,
			errors: [{ message: detail }],
			status: res.status
		};
	}

	return { data: body.data, errors: body.errors, status: res.status };
}
