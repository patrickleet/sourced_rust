import type {
	ReplicaAuthoritativeScope,
	ReplicaDehydratedState
} from '../types.js';

export const REPLICA_OFFLINE_COMMAND_OUTBOX_SUPPORTED = false as const;

export type ReplicaPersistenceModelPolicy = {
	/**
	 * Durable storage is an explicit per-model choice. `memory-only` is always
	 * honored, even when the model is otherwise non-sensitive.
	 */
	readonly retention: 'persist-confirmed' | 'memory-only';
	/**
	 * Sensitivity must be classified explicitly. Sensitive or unclassified
	 * models are never written to durable storage.
	 */
	readonly sensitive: boolean;
};

export type ReplicaPersistencePolicy = {
	readonly models: Readonly<Record<string, ReplicaPersistenceModelPolicy>>;
};

export type ReplicaIndexedDbFactory = Pick<IDBFactory, 'open'>;

export type ReplicaIndexedDbPersistenceOptions = {
	/**
	 * Defaults to the browser's IndexedDB factory. Supplying a factory makes the
	 * boundary deterministic in tests and non-window runtimes.
	 */
	readonly indexedDB?: ReplicaIndexedDbFactory;
	readonly databaseName?: string;
	/**
	 * Missing models, missing policy, `memory-only`, and `sensitive: true` all
	 * fail closed. A model persists only when both fields explicitly allow it.
	 */
	readonly policy?: ReplicaPersistencePolicy;
};

export interface ReplicaIndexedDbPersistence {
	readonly supportsOfflineCommandOutbox: false;

	/**
	 * Validate, policy-filter, and persist one confirmed dehydration envelope.
	 *
	 * Returns false when policy leaves no durable records or causal fences.
	 * Malformed caller input rejects rather than entering durable storage.
	 */
	save(state: ReplicaDehydratedState): Promise<boolean>;

	/**
	 * Restore only after the caller has independently obtained the exact current
	 * server scope. Corrupt, unsupported, or mismatched entries are deleted and
	 * return undefined.
	 */
	restore(
		authoritativeScope: ReplicaAuthoritativeScope
	): Promise<ReplicaDehydratedState | undefined>;

	/** Delete confirmed state for exactly one independently authoritative scope. */
	discard(authoritativeScope: ReplicaAuthoritativeScope): Promise<void>;

	/**
	 * Close this instance's database handle. Persistence deliberately performs
	 * no BroadcastChannel, leader election, or active multi-tab synchronization.
	 */
	close(): void;
}
