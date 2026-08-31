import { createDistributedSvelteKitServer } from '@hops-ops/distributed/sveltekit';

import { DISTRIBUTED_BOUNDARY_OPERATIONS } from '$distributed';
import { engineRoleFromGroups } from '$lib/roles';
import { graphqlHttpUrl } from '$lib/server/graphql';

import type { LayoutServerLoad } from './$types';

type LoadEvent = Parameters<LayoutServerLoad>[0];
type Session = NonNullable<
	Awaited<ReturnType<LoadEvent['locals']['auth']>>
>;

/**
 * One root loader owns every compiler-discovered user-safe `@load` operation.
 * A fresh replica is created per request and no GraphQL work runs for routes
 * absent from the generated registry.
 */
const distributed = createDistributedSvelteKitServer<Session, LoadEvent>({
	boundaries: DISTRIBUTED_BOUNDARY_OPERATIONS,
	getSession: (event) => event.locals.auth(),
	getRole: (session) => engineRoleFromGroups(session?.user?.groups),
	getUrl: graphqlHttpUrl
});

/**
 * Unauthenticated traffic skips the portable user GraphQL surface (no Bearer /
 * empty identity cannot open e2e-ui). Lobby chat SSR uses the nested public
 * client under routes/chat when there is no session.
 */
export const load: LayoutServerLoad = (async (event) => {
	const session = await event.locals.auth();
	if (!session?.user) {
		return {
			session: null,
			accessToken: null,
			engineRole: null,
			gqlError: null
		};
	}
	return distributed.load(event);
}) satisfies LayoutServerLoad;
