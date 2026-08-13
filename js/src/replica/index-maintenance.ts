/** Index maintenance registry and plan evaluation; implementation in ./index-maintenance/. */
export {
	createReplicaIndexMaintenanceRegistry,
	formatReplicaIndexStaleReason
} from './index-maintenance/index.js';
export type {
	ReplicaIndexDependencyChange,
	ReplicaIndexMaintenanceDecision,
	ReplicaIndexMaintenanceIndex,
	ReplicaIndexMaintenanceReason,
	ReplicaIndexMaintenanceReasonCode,
	ReplicaIndexMaintenanceRecord,
	ReplicaIndexMaintenanceRegistry,
	ReplicaIndexMaintenanceSnapshot,
	ReplicaIndexPlanRegistration,
	ReplicaIndexRecordChange,
	ReplicaIndexRelationshipChange,
	ReplicaIndexSemanticChange,
	ReplicaIndexSemanticLayer
} from './index-maintenance/index.js';
