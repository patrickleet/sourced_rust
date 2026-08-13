/** Replica diagnostics sink and artifact inspection; implementation in ./diagnostics/. */
export {
	createReplicaDevelopmentCapability,
	createReplicaDiagnostics,
	inspectReplicaCommandArtifact,
	inspectReplicaOperationArtifact
} from './diagnostics/index.js';
export type {
	ReplicaArtifactSourceLocation,
	ReplicaCommandArtifactInspection,
	ReplicaCommandEffectInspection,
	ReplicaDevelopmentCapability,
	ReplicaDiagnosticEvent,
	ReplicaDiagnosticEventInput,
	ReplicaDiagnosticFieldValueContext,
	ReplicaDiagnosticFieldValuePolicy,
	ReplicaDiagnosticIndex,
	ReplicaDiagnosticIndexInput,
	ReplicaDiagnosticLayer,
	ReplicaDiagnosticLayerInput,
	ReplicaDiagnosticReceipt,
	ReplicaDiagnosticReceiptExpectationInput,
	ReplicaDiagnosticReceiptInput,
	ReplicaDiagnosticRecord,
	ReplicaDiagnosticRecordInput,
	ReplicaDiagnosticReasonContext,
	ReplicaDiagnosticReasonPolicy,
	ReplicaDiagnostics,
	ReplicaDiagnosticsOptions,
	ReplicaDiagnosticsSink,
	ReplicaDiagnosticsSnapshot,
	ReplicaDiagnosticScopeInput,
	ReplicaDiagnosticStateInput,
	ReplicaOperationArtifactInspection,
	ReplicaOperationIndexInspection,
	ReplicaOperationInjectedFieldInspection
} from './diagnostics/index.js';
