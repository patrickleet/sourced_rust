import type { CacheIndex, CacheReader } from '../internal/cache-engine.js';
import type { GraphqlVariables } from '../types.js';
import { replicaIndexKey, resolveArguments } from './identity.js';
import type {
	ReplicaEntitySelection,
	ReplicaOperationArtifact,
	ReplicaRelationshipSelection,
	ReplicaRootSelection,
	ReplicaSparse
} from './types.js';

export type MaterializedReplicaResult<TData> = {
	readonly data: ReplicaSparse<TData>;
	readonly complete: boolean;
	readonly stale: boolean;
	readonly identitySignature: string;
};

type MaterializedEntity = {
	value?: Readonly<Record<string, unknown>>;
	complete: boolean;
	stale: boolean;
	identitySignature: string;
};

type MaterializedBranch = {
	value?: unknown;
	present: boolean;
	complete: boolean;
	stale: boolean;
	identitySignature: string;
};

export function materializeReplicaOperation<
	TData,
	TVariables extends GraphqlVariables
>(
	reader: CacheReader,
	artifact: ReplicaOperationArtifact<TData, TVariables>,
	variables: TVariables
): MaterializedReplicaResult<TData> {
	const output: Record<string, unknown> = {};
	let complete = true;
	let stale = false;
	const signatures: string[] = [];
	for (const root of artifact.roots) {
		const argumentsValue = resolveArguments(root.arguments, variables);
		const indexKey = replicaIndexKey({ field: root.field, arguments: argumentsValue });
		const index = reader.index(indexKey);
		const branch = materializeBranch(reader, root, index, variables);
		if (root.expose !== false && branch.present) {
			defineOutputValue(output, root.responseKey, branch.value);
		}
		complete &&= branch.complete;
		stale ||= branch.stale;
		signatures.push(`${indexKey}:${branch.identitySignature}`);
	}
	return Object.freeze({
		data: Object.freeze(output) as ReplicaSparse<TData>,
		complete,
		stale,
		identitySignature: signatures.join('|')
	});
}

function materializeBranch(
	reader: CacheReader,
	selection: ReplicaRootSelection | ReplicaRelationshipSelection,
	index: CacheIndex | undefined,
	variables: GraphqlVariables
): MaterializedBranch {
	if (!index || !index.metadata) {
		return {
			present: false,
			complete: false,
			stale: false,
			identitySignature: 'missing-index'
		};
	}
	const stale = index.metadata.staleReason !== undefined;
	const indexComplete = index.complete && !stale;
	if (index.metadata.nullValue) {
		return {
			value: null,
			present: true,
			complete: indexComplete,
			stale,
			identitySignature: `null:${index.metadata.staleReason ?? ''}`
		};
	}

	if (selection.cardinality === 'one') {
		const key = index.records[0];
		if (key === undefined) {
			return {
				present: false,
				complete: false,
				stale,
				identitySignature: `missing-record:${index.metadata.staleReason ?? ''}`
			};
		}
		const entity = materializeEntity(reader, selection.selection, key, variables);
		return {
			value: entity.value,
			present: entity.value !== undefined,
			complete: indexComplete && index.records.length === 1 && entity.complete,
			stale: stale || entity.stale,
			identitySignature: entity.identitySignature
		};
	}

	const values: Readonly<Record<string, unknown>>[] = [];
	let childrenComplete = true;
	let childrenStale = false;
	const signatures: string[] = [];
	for (const key of index.records) {
		const entity = materializeEntity(reader, selection.selection, key, variables);
		if (entity.value !== undefined) values.push(entity.value);
		else childrenComplete = false;
		childrenComplete &&= entity.complete;
		childrenStale ||= entity.stale;
		signatures.push(entity.identitySignature);
	}
	return {
		value: Object.freeze(values),
		present: true,
		complete: indexComplete && childrenComplete,
		stale: stale || childrenStale,
		identitySignature: signatures.join(',')
	};
}

function materializeEntity(
	reader: CacheReader,
	selection: ReplicaEntitySelection,
	key: string,
	variables: GraphqlVariables
): MaterializedEntity {
	const record = reader.recordMeta(key);
	if (!record) {
		return {
			complete: false,
			stale: false,
			identitySignature: `${key}:missing`
		};
	}
	const output: Record<string, unknown> = {};
	let complete = true;
	let stale = false;
	const nestedSignatures: string[] = [];
	for (const field of selection.fields) {
		const presence = reader.field(key, field.field);
		if (!presence.present) {
			complete = false;
			continue;
		}
		if (field.expose !== false) {
			defineOutputValue(output, field.responseKey, presence.value);
		}
	}

	for (const relationship of selection.relationships ?? []) {
		const relationshipResult = materializeRelationship(
			reader,
			key,
			record.incarnation,
			relationship,
			variables
		);
		if (relationship.expose !== false && relationshipResult.present) {
			defineOutputValue(output, relationship.responseKey, relationshipResult.value);
		}
		complete &&= relationshipResult.complete;
		stale ||= relationshipResult.stale;
		nestedSignatures.push(`${relationship.field}:${relationshipResult.identitySignature}`);
	}

	return {
		value: Object.freeze(output),
		complete,
		stale,
		identitySignature: `${key}@${record.incarnation}[${nestedSignatures.join('|')}]`
	};
}

function materializeRelationship(
	reader: CacheReader,
	parentKey: string,
	parentIncarnation: string,
	selection: ReplicaRelationshipSelection,
	variables: GraphqlVariables
): MaterializedBranch {
	const argumentsValue = resolveArguments(selection.arguments, variables);
	const indexKey = replicaIndexKey({
		parent: parentKey,
		field: selection.field,
		arguments: argumentsValue
	});
	const index = reader.index(indexKey);
	if (index?.metadata?.parentIncarnation !== parentIncarnation) {
		return {
			present: false,
			complete: false,
			stale: index !== undefined,
			identitySignature: `parent-incarnation-mismatch:${parentIncarnation}`
		};
	}
	return materializeBranch(reader, selection, index, variables);
}

function defineOutputValue(
	output: Record<string, unknown>,
	key: string,
	value: unknown
): void {
	Object.defineProperty(output, key, {
		value,
		enumerable: true,
		configurable: false,
		writable: false
	});
}
