import { getContext, setContext } from 'svelte';

import type {
	ReplicaOperationArtifact,
	ReplicaSnapshot
} from '../replica/index.js';
import type { GraphqlVariables } from '../types.js';
import type {
	DistributedSvelteKitClient,
	SveltekitBoundOperation
} from './replica.js';

const DISTRIBUTED_CLIENT_CONTEXT = Symbol(
	'@hops-ops/distributed/sveltekit/client'
);

type AnyDistributedSvelteKitClient = DistributedSvelteKitClient<unknown>;

/**
 * Install one client in the current Svelte component tree.
 *
 * The context key is module-global, but the client value is not: Svelte owns
 * it per component tree/request. A nested elevated layout may deliberately
 * replace the nearest user-safe client.
 */
export function provideDistributedSvelteKitClient<TCommands>(
	client: DistributedSvelteKitClient<TCommands>
): DistributedSvelteKitClient<TCommands> {
	if (
		client === null ||
		typeof client !== 'object' ||
		typeof client.operation !== 'function'
	) {
		throw new TypeError(
			'provideDistributedSvelteKitClient requires a Distributed SvelteKit client'
		);
	}
	setContext(
		DISTRIBUTED_CLIENT_CONTEXT,
		client as AnyDistributedSvelteKitClient
	);
	return client;
}

/**
 * Resolve the nearest client during component initialization.
 *
 * This intentionally calls Svelte `getContext` each time. Generated modules
 * may contain static operation wrappers, but never capture a client or command
 * tree at module evaluation time.
 */
export function useDistributedSvelteKitClient<
	TCommands = Readonly<Record<never, never>>
>(): DistributedSvelteKitClient<TCommands> {
	const client = getContext<AnyDistributedSvelteKitClient | undefined>(
		DISTRIBUTED_CLIENT_CONTEXT
	);
	if (client === undefined) {
		throw new Error(
			'Distributed client is missing from the active Svelte component tree; call generated provideDistributed(...) in a parent layout'
		);
	}
	return client as DistributedSvelteKitClient<TCommands>;
}

/** Resolve the nearest generated command surface without a global proxy. */
export function useDistributedSvelteKitCommands<TCommands>(): TCommands {
	return useDistributedSvelteKitClient<TCommands>().commands;
}

/**
 * Define one SSR-safe generated operation wrapper.
 *
 * The wrapper stores only immutable compiler output. `use()` and `read()`
 * resolve the nearest tree-local client when the component calls them.
 */
export function defineDistributedSvelteKitOperation<
	TData,
	TVariables extends GraphqlVariables
>(
	artifact: ReplicaOperationArtifact<TData, TVariables>
): SveltekitBoundOperation<TData, TVariables> {
	const use = ((...args: unknown[]) => {
		const operation =
			useDistributedSvelteKitClient<unknown>().operation(artifact);
		const invoke = operation.use as (
			...values: unknown[]
		) => ReturnType<SveltekitBoundOperation<TData, TVariables>['use']>;
		return invoke(...args);
	}) as SveltekitBoundOperation<TData, TVariables>['use'];

	return Object.freeze({
		artifact,
		use,
		read(variables: TVariables): ReplicaSnapshot<TData> {
			return useDistributedSvelteKitClient<unknown>()
				.operation(artifact)
				.read(variables);
		},
		prefetch(variables: TVariables): Promise<void> {
			return useDistributedSvelteKitClient<unknown>()
				.operation(artifact)
				.prefetch(variables);
		}
	});
}
