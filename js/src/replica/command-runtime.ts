/** Public entry for command runtime; implementation lives in ./command-runtime/. */
export {
	createReplicaCommandRuntime,
	ReplicaCommandRuntimeError,
	replicaCommandAuthority,
	replicaCommandDirectProjection,
	replicaCommandProjectionDelta,
	replicaCommandProjectedLifecycle,
	replicaCommandProjectedLifecycleOf,
	replicaResultObservation
} from './command-runtime/index.js';
export type {
	ReplicaBoundCommand,
	ReplicaBoundCommands,
	ReplicaCommandAuthorityHost,
	ReplicaCommandAuthorityRegistration,
	ReplicaCommandAuthoritySnapshot,
	ReplicaCommandCallOptions,
	ReplicaCommandDirectProjection,
	ReplicaCommandProjectedOutcome,
	ReplicaCommandReceipt,
	ReplicaCommandRecoveryReceipt,
	ReplicaCommandRuntime,
	ReplicaCommandRuntimeErrorCode,
	ReplicaCommandRuntimeOptions,
	ReplicaCommandStatus,
	ReplicaCommandStatusArtifact,
	ReplicaCommandStatusRequest,
	ReplicaCommandSurfaceContract,
	ReplicaCommandTransport,
	ReplicaCommandTransportRequest,
	ReplicaCommandTransportResult,
	ReplicaResultObservationRegistration
} from './command-runtime/index.js';
