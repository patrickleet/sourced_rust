/**
 * SSR load helper for co-located resources.
 * `.server.ts` so private env / serverGraphql never enter the client bundle.
 */
import type { ServerLoadEvent } from '@sveltejs/kit';
import type { TypedDocumentNode } from '@graphql-typed-document-node/core';
import { engineRoleFromGroups } from '$lib/roles';
import { serverGraphql } from '$lib/server/graphql';
import { authFromPageData } from './auth-from-page.ts';
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
		const accessToken = session?.accessToken ?? null;
		const engineRole = engineRoleFromGroups(session?.user?.groups);
		const auth = authFromPageData({ accessToken, session, engineRole });

		const result = await serverGraphql<TData>(document, auth);

		return {
			session,
			accessToken,
			engineRole,
			gqlError:
				result.errors?.[0]?.message ?? (result.status >= 400 ? `HTTP ${result.status}` : null),
			gqlStatus: result.status,
			...map(result.data, result)
		};
	};
}
