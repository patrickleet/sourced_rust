import {
	createDistributedSvelteKitServer,
	type SveltekitDistributedPageData
} from '@hops-ops/distributed/sveltekit';

import { DISTRIBUTED_BOUNDARY_OPERATIONS } from '$distributed/public';
import { graphqlHttpUrl } from '$lib/server/graphql';

import type { LayoutServerLoad } from './$types';

type LoadEvent = Parameters<LayoutServerLoad>[0];
type Session = NonNullable<Awaited<ReturnType<LoadEvent['locals']['auth']>>>;

/**
 * Guest lobby SSR: e2e-ui-public + empty identity (anonymous privilege pack).
 * Signed-in visitors inherit the root user client hydration from +layout.server.
 */
const publicDistributed = createDistributedSvelteKitServer<Session, LoadEvent>({
	boundaries: DISTRIBUTED_BOUNDARY_OPERATIONS,
	getSession: async () => null,
	getRole: () => null,
	getUrl: graphqlHttpUrl
});

export const load: LayoutServerLoad = (async (event) => {
	const session = await event.locals.auth();
	if (session?.user) {
		// Parent already loaded user-surface ChatMessages for this route.
		return {
			session,
			accessToken: session.accessToken ?? null,
			engineRole: null,
			gqlError: null
		};
	}
	return publicDistributed.load(event);
}) satisfies (event: LoadEvent) => Promise<SveltekitDistributedPageData & { gqlError: string | null }>;
