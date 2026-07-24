import { error } from '@sveltejs/kit';
import {
	createDistributedSvelteKitServer,
	type SveltekitDistributedPageData
} from '@hops-ops/distributed/sveltekit';

import { DISTRIBUTED_ROUTE_OPERATIONS } from '$distributed/admin';
import { engineRoleFromGroups, isAdminEngineRole } from '$lib/roles';
import { graphqlHttpUrl } from '$lib/server/graphql';

import type { LayoutServerLoad } from './$types';

type LoadEvent = Parameters<LayoutServerLoad>[0];
type Session = NonNullable<
	Awaited<ReturnType<LoadEvent['locals']['auth']>>
>;

/**
 * Elevated GraphQL is a separate generated surface and nested client boundary.
 * Role failure happens before the server adapter can issue any GraphQL request.
 */
const distributed = createDistributedSvelteKitServer<Session, LoadEvent>({
	routes: DISTRIBUTED_ROUTE_OPERATIONS,
	getSession: (event) => event.locals.auth(),
	getRole: (session) => {
		const role = engineRoleFromGroups(session?.user?.groups);
		if (!isAdminEngineRole(role)) {
			error(
				403,
				'Admin role required — sign in as admin (Zitadel: admin / Password1!)'
			);
		}
		return role;
	},
	getUrl: graphqlHttpUrl
});

export const load: LayoutServerLoad = distributed.load satisfies (
	event: LoadEvent
) => Promise<SveltekitDistributedPageData>;
