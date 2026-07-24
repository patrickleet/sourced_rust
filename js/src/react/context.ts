'use client';

import {
	createContext,
	createElement,
	useContext,
	type ReactElement,
	type ReactNode
} from 'react';

import type { DistributedReplica } from '../replica/types.js';

const DistributedReplicaContext = createContext<DistributedReplica | undefined>(
	undefined
);

export type DistributedProviderProps = {
	/**
	 * One framework-neutral replica for this browser authorization lifecycle, or
	 * one fresh replica for this server-render request.
	 */
	readonly replica: DistributedReplica;
	readonly children?: ReactNode;
};

/**
 * Makes an existing framework-neutral replica available to React.
 *
 * The provider deliberately does not create, hydrate, authorize, or retain a
 * replica. Those lifecycles stay in the shared core and application
 * composition, which keeps SSR request isolation explicit.
 */
export function DistributedProvider({
	replica,
	children
}: DistributedProviderProps): ReactElement {
	if (replica === undefined || replica === null) {
		throw new TypeError('DistributedProvider requires a replica');
	}
	return createElement(DistributedReplicaContext.Provider, { value: replica }, children);
}

/** Read the exact replica supplied by the nearest DistributedProvider. */
export function useDistributedReplica(): DistributedReplica {
	const replica = useContext(DistributedReplicaContext);
	if (replica === undefined) {
		throw new Error(
			'useDistributedReplica must be used inside a DistributedProvider'
		);
	}
	return replica;
}
