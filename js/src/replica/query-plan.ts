/** Query plan evaluation; implementation lives in ./query-plan/. */
export {
	compareReplicaOrder,
	decideReplicaPaginationMaintenance,
	evaluateReplicaFilter
} from './query-plan/index.js';
export type {
	ReplicaFilterEvaluation,
	ReplicaFilterEvaluationOptions,
	ReplicaOrderComparison,
	ReplicaPaginationChange,
	ReplicaPaginationMaintenanceDecision,
	ReplicaQueryPlanPath,
	ReplicaQueryPlanReason,
	ReplicaQueryPlanReasonCode,
	ReplicaRelationshipFilterRequest
} from './query-plan/index.js';
