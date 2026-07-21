/** App-owned SSR wiring over the published SvelteKit load adapter. */
import { createLoadQuery } from '@hops-ops/distributed/sveltekit';
import { engineRoleFromGroups } from '$lib/roles';
import { serverGraphql } from '$lib/server/graphql';

type AuthSession = {
	accessToken?: string | null;
	user?: {
		id?: string | null;
		name?: string | null;
		email?: string | null;
		username?: string | null;
		groups?: string[];
	} | null;
};

type AuthLocals = {
	auth: () => Promise<AuthSession | null>;
};

export const loadQuery = createLoadQuery<AuthSession>({
	getSession: (event) => (event.locals as AuthLocals).auth(),
	getRole: (session) => engineRoleFromGroups(session?.user?.groups),
	request: (document, auth, variables) =>
		serverGraphql(document, { ...auth, variables })
});
