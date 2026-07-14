/**
 * SSR load helper for co-located resources.
 * `.server.ts` so private env / serverGraphql never enter the client bundle.
 * Accepts string or TypedDocumentNode (from co-located .gql codegen).
 */
import type { ServerLoadEvent } from '@sveltejs/kit';
import type { TypedDocumentNode } from '@graphql-typed-document-node/core';
import { engineRoleFromGroups } from '$lib/roles';
import { serverGraphql } from '$lib/server/graphql';
import type { GqlDocument } from './document.ts';
import type { GqlResult } from './types.ts';

type AuthLocals = {
	auth: () => Promise<{
		accessToken?: string | null;
		user?: { id?: string; groups?: string[] } | null;
	} | null>;
};

/**
 * Build a PageServerLoad that seeds from `document` (resource.query).
 * Always returns session + accessToken + engineRole for the browser client binder.
 */
export function loadQuery<TData, TMapped extends Record<string, unknown>>(
	document: GqlDocument | TypedDocumentNode<TData, Record<string, unknown>>,
	map: (data: TData | undefined, result: GqlResult<TData>) => TMapped
) {
	return async (event: ServerLoadEvent) => {
		const locals = event.locals as AuthLocals;
		const session = await locals.auth();
		const accessToken = session?.accessToken;
		const role = engineRoleFromGroups(session?.user?.groups);

		const result = await serverGraphql<TData>(document, {
			accessToken,
			userId: accessToken ? undefined : session?.user?.id,
			role
		});

		return {
			session,
			accessToken: accessToken ?? null,
			engineRole: role,
			gqlError:
				result.errors?.[0]?.message ?? (result.status >= 400 ? `HTTP ${result.status}` : null),
			gqlStatus: result.status,
			...map(result.data, result)
		};
	};
}
