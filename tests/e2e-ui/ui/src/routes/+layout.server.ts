import {
	createDistributedSvelteKitServer,
	type SveltekitDistributedPageData
} from '@hops-ops/distributed/sveltekit';

import { DISTRIBUTED_ROUTE_OPERATIONS } from '$distributed';
import { CHAT_PAGE_SIZE } from '$lib/chat/lobby-log';
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
	routes: DISTRIBUTED_ROUTE_OPERATIONS,
	getSession: (event) => event.locals.auth(),
	getRole: (session) => engineRoleFromGroups(session?.user?.groups),
	getUrl: graphqlHttpUrl,
	// ChatMessages requires limit/offset; seed the live newest page on SSR.
	variables: {
		ChatMessages: () => ({ limit: CHAT_PAGE_SIZE, offset: 0 })
	}
});

export const load: LayoutServerLoad = distributed.load satisfies (
	event: LoadEvent
) => Promise<SveltekitDistributedPageData>;
