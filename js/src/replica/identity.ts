/** Replica identity/keys and variable codec; implementation in ./identity/. */
export {
	canonicalCacheValue,
	canonicalVariables,
	canonicalizeOperationVariables,
	cloneJsonObject,
	cloneJsonValue,
	coverageFromArtifact,
	replicaIndexKey,
	replicaRecordKey,
	resolveArguments,
	resolveReplicaArgumentValue
} from './identity/index.js';
