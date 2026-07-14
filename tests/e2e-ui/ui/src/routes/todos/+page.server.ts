import type { PageServerLoad } from './$types';
import { engineRoleFromGroups } from '$lib/roles';
import { TODOS_QUERY } from '$lib/gql/documents';
import { serverGraphql } from '$lib/server/graphql';

type Todo = {
	todo_id: string;
	owner_id: string;
	title: string;
	status: string;
};

/**
 * SSR seed only — same TODOS_QUERY the browser uses for reconcile.
 * Mutations run in the browser via browserGraphql (POST /graphql).
 */
export const load: PageServerLoad = async ({ locals }) => {
	const session = await locals.auth();
	const accessToken = session?.accessToken;
	const role = engineRoleFromGroups(session?.user?.groups);

	const result = await serverGraphql<{ todos: Todo[] }>(TODOS_QUERY, {
		accessToken,
		userId: accessToken ? undefined : session?.user?.id,
		role
	});

	return {
		session,
		/** For browser GraphQL (same token SSR used). */
		accessToken: accessToken ?? null,
		engineRole: role,
		todos: result.data?.todos ?? [],
		gqlError: result.errors?.[0]?.message ?? (result.status >= 400 ? `HTTP ${result.status}` : null),
		gqlStatus: result.status
	};
};
