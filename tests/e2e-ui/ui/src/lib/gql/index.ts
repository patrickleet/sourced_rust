/** App composition: generated service commands + the published SvelteKit adapter. */
import { createUseGraphql } from '@hops-ops/distributed/sveltekit';
import { bindCommands, COMMANDS } from '$lib/api/commands.generated';
import { commandPolicies } from '$lib/api/commands.policies.generated';

export const useGraphql = createUseGraphql<typeof COMMANDS>({
	bindCommands,
	policies: commandPolicies
});

export type AppGraphqlClient = ReturnType<typeof useGraphql>;

// One app import for pages; reusable behavior remains owned by the npm package.
export * from '@hops-ops/distributed';
export { authFromPageData, type PageGraphqlData } from '@hops-ops/distributed/sveltekit';
export { commandPolicies } from '$lib/api/commands.policies.generated';
