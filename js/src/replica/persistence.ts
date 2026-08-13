/** Confirmed-state IndexedDB persistence; implementation in ./persistence/. */
export {
	createReplicaIndexedDbPersistence,
	REPLICA_OFFLINE_COMMAND_OUTBOX_SUPPORTED
} from './persistence/index.js';
export type {
	ReplicaIndexedDbFactory,
	ReplicaIndexedDbPersistence,
	ReplicaIndexedDbPersistenceOptions,
	ReplicaPersistenceModelPolicy,
	ReplicaPersistencePolicy
} from './persistence/index.js';
