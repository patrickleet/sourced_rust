import {
	distributedSvelteKit as createDistributedSvelteKitPlugin,
	distributedSvelteKitAliases as createDistributedSvelteKitAliases,
	type DistributedSvelteKitViteOptions,
	type DistributedSvelteKitVitePlugin
} from './vite.js';

export {
	analyzeDistributedSvelteKitBoundaries,
	distributedGraphqlProxy,
	validateDistributedSvelteKitBoundaryPlan
} from './vite.js';

export type {
	DistributedGraphqlProxyOptions,
	DistributedIslandInventory,
	DistributedIslandPlanInput,
	DistributedSvelteKitBoundary,
	DistributedSvelteKitBoundaryAnalysisClient,
	DistributedSvelteKitBoundaryAnalysisOptions,
	DistributedSvelteKitBoundaryOccurrence,
	DistributedSvelteKitBoundaryPlan,
	DistributedSvelteKitBoundaryRegistration,
	DistributedSvelteKitClientCompiler,
	DistributedSvelteKitManifestSource,
	DistributedSvelteKitViteOptions,
	DistributedSvelteKitVitePlugin
} from './vite.js';

const requireApplicationLifecycle = (): void => {
	if (process.env.DISTRIBUTED_LIFECYCLE_OWNS_CLIENT_COMPILE !== '1') {
		throw new Error(
			'Distributed SvelteKit clients are application lifecycle outputs; run the UI through `distributed dev` or `distributed build`'
		);
	}
};

/** Configure Vite to consume generated clients from the active application generation. */
export function distributedSvelteKit(
	options: DistributedSvelteKitViteOptions
): DistributedSvelteKitVitePlugin {
	requireApplicationLifecycle();
	return createDistributedSvelteKitPlugin(options);
}

/** Resolve generated-client aliases from the active application generation. */
export function distributedSvelteKitAliases(
	options: Pick<DistributedSvelteKitViteOptions, 'cwd' | 'clients'>
): Record<string, string> {
	requireApplicationLifecycle();
	return createDistributedSvelteKitAliases(options);
}
