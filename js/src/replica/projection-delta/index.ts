export {
	canonicalCommandProjectionMetadata,
	canonicalProjectionDelta,
	parseCommandProjectionMetadata,
	parseProjectionDelta,
	ProjectionDeltaValidationError,
	validateCommandProjectionArtifact,
	validateProjectionMetadataAuthority
} from './validate.js';
export {
	deltaValue,
	operationsFromProjectionDelta,
	prepareCommandProjection
} from './resolve.js';
export type {
	AppliedProjectionDelta,
	CommandProjectionMetadata,
	PreparedCommandProjection,
	PreparedProjectionOperation,
	PreparedProjectionScope,
	ProjectionCapabilityArm,
	ProjectionCapabilityMutation,
	ProjectionDelta,
	ProjectionDeltaMutation,
	ProjectionDeltaRecoveryTarget,
	ProjectionDeltaScope,
	ProjectionDeltaValue,
	ProjectionPreviewPureReduce,
	ProjectionPreviewScope,
	ProjectionPreviewValue,
	ReplicaCommandProjection,
	ReplicaPureFunction
} from './types.js';
