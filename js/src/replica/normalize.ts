import type {
	BaseCacheWriter,
	CacheIndexMetadata,
	CacheValue,
	RecordKey,
	Revision
} from '../internal/cache-engine.js';
import type { GqlError, GraphqlVariables } from '../types.js';
import {
	cloneJsonValue,
	coverageFromArtifact,
	replicaIndexKey,
	replicaRecordKey,
	resolveArguments
} from './identity.js';
import type {
	ReplicaEntitySelection,
	ReplicaOperationArtifact,
	ReplicaRelationshipSelection,
	ReplicaResultEnvelope,
	ReplicaRootSelection
} from './types.js';

export type ReplicaNormalizationSummary = {
	readonly wrote: boolean;
	readonly partial: boolean;
	readonly indexKeys: readonly string[];
};

type NormalizedBranch = {
	keys: RecordKey[];
	nullValue: boolean;
	partial: boolean;
};

type ErrorPaths = {
	global: boolean;
	paths: readonly (readonly (string | number)[])[];
};

export function normalizeReplicaResult<
	TData,
	TVariables extends GraphqlVariables
>(
	writer: BaseCacheWriter,
	artifact: ReplicaOperationArtifact<TData, TVariables>,
	variables: TVariables,
	envelope: ReplicaResultEnvelope<TData>
): ReplicaNormalizationSummary {
	validateArtifact(artifact);
	const errors = collectErrorPaths(envelope.errors ?? []);
	if (errors.global || envelope.data === undefined) {
		return Object.freeze({ wrote: false, partial: errors.global, indexKeys: [] });
	}
	if (envelope.data === null && (envelope.errors?.length ?? 0) > 0) {
		// `data: null` is a valid GraphQL error result after non-null propagation.
		// The ingress coordinator will stale the operation roots without replacing
		// the last known-good memberships.
		return Object.freeze({ wrote: false, partial: true, indexKeys: [] });
	}
	if (envelope.data === null || !isObject(envelope.data)) {
		throw new TypeError(`operation ${artifact.id} returned a non-object GraphQL data value`);
	}

	let wrote = false;
	let partial = false;
	const indexKeys: string[] = [];
	for (const root of artifact.roots) {
		const path: readonly (string | number)[] = [root.responseKey];
		const hasValue = Object.prototype.hasOwnProperty.call(envelope.data, root.responseKey);
		const blocked = pathBlocked(errors, path);
		const argumentsValue = resolveArguments(root.arguments, variables);
		const key = replicaIndexKey({ field: root.field, arguments: argumentsValue });
		const hasErrors = pathHasErrors(errors, path);
		const rawValue = hasValue ? envelope.data[root.responseKey] : undefined;
		if (blocked || !hasValue || (rawValue === null && hasErrors)) {
			wrote =
				writer.markIndexStale(
					key,
					hasErrors ? 'graphql-partial-error' : 'incomplete-result',
					envelope.revision
				) || wrote;
			indexKeys.push(key);
			partial = true;
			continue;
		}
		const value = rawValue;
		const branch = normalizeBranch(
			writer,
			root,
			value,
			path,
			envelope.revision,
			variables,
			errors
		);
		const rootPartial = blocked || !hasValue || branch.partial || hasErrors;
		const metadata = indexMetadata(
			root,
			argumentsValue,
			branch,
			rootPartial,
			hasErrors
		);
		if (
			writer.writeIndex({
				key,
				revision: envelope.revision,
				records: branch.keys,
				complete: !rootPartial,
				metadata
			})
		) {
			wrote = true;
		}
		indexKeys.push(key);
		partial ||= rootPartial;
	}
	return Object.freeze({
		wrote,
		partial,
		indexKeys: Object.freeze(indexKeys)
	});
}

function normalizeBranch(
	writer: BaseCacheWriter,
	selection: ReplicaRootSelection | ReplicaRelationshipSelection,
	value: unknown,
	path: readonly (string | number)[],
	snapshotRevision: Revision,
	variables: GraphqlVariables,
	errors: ErrorPaths
): NormalizedBranch {
	if (value === null) return { keys: [], nullValue: true, partial: false };
	if (value === undefined) return { keys: [], nullValue: false, partial: true };
	if (selection.cardinality === 'one') {
		const entity = normalizeEntity(
			writer,
			selection.selection,
			value,
			path,
			snapshotRevision,
			variables,
			errors
		);
		return {
			keys: entity.key === undefined ? [] : [entity.key],
			nullValue: false,
			partial: entity.partial
		};
	}
	if (!Array.isArray(value)) {
		return { keys: [], nullValue: false, partial: true };
	}
	const keys: RecordKey[] = [];
	let partial = false;
	for (const [index, entry] of value.entries()) {
		if (entry === null || entry === undefined) {
			partial = true;
			continue;
		}
		const entity = normalizeEntity(
			writer,
			selection.selection,
			entry,
			[...path, index],
			snapshotRevision,
			variables,
			errors
		);
		if (entity.key === undefined) partial = true;
		else keys.push(entity.key);
		partial ||= entity.partial;
	}
	return { keys, nullValue: false, partial };
}

function normalizeEntity(
	writer: BaseCacheWriter,
	selection: ReplicaEntitySelection,
	value: unknown,
	path: readonly (string | number)[],
	snapshotRevision: Revision,
	variables: GraphqlVariables,
	errors: ErrorPaths
): { key?: RecordKey; partial: boolean } {
	if (!isObject(value) || pathBlocked(errors, path)) return { partial: true };

	const fields: Record<string, CacheValue> = Object.create(null) as Record<
		string,
		CacheValue
	>;
	let partial = false;
	for (const field of selection.fields) {
		const fieldPath = [...path, field.responseKey];
		if (
			pathBlocked(errors, fieldPath) ||
			!Object.prototype.hasOwnProperty.call(value, field.responseKey)
		) {
			partial = true;
			continue;
		}
		const next = cloneJsonValue(value[field.responseKey]);
		if (Object.prototype.hasOwnProperty.call(fields, field.field)) {
			if (!deepEqual(fields[field.field], next)) {
				throw new TypeError(
					`operation aliases disagree for ${selection.model.id}.${field.field}`
				);
			}
			continue;
		}
		fields[field.field] = next;
	}

	const identity: CacheValue[] = [];
	for (const field of selection.model.identityFields) {
		if (!Object.prototype.hasOwnProperty.call(fields, field) || fields[field] === null) {
			return { partial: true };
		}
		identity.push(fields[field]!);
	}
	const key = replicaRecordKey(selection.model, identity);
	if (
		(selection.revisionResponseKey !== undefined &&
			pathHasErrors(errors, [...path, selection.revisionResponseKey])) ||
		(selection.incarnationResponseKey !== undefined &&
			pathHasErrors(errors, [...path, selection.incarnationResponseKey]))
	) {
		// Injected clock fields are wire metadata. If GraphQL reports one as
		// unavailable, keep the prior entity and memberships instead of guessing a
		// clock or rejecting an otherwise valid partial-error envelope.
		return { key, partial: true };
	}
	const revision = responseRevision(
		value,
		selection.revisionResponseKey,
		snapshotRevision,
		'row revision'
	);
	if (revision === undefined) throw new TypeError('row revision is missing');
	const incarnation = responseRevision(
		value,
		selection.incarnationResponseKey,
		undefined,
		'row incarnation'
	);
	// Establish (or validate) the parent lifecycle before attaching exact
	// relationship indexes. The enclosing engine batch still makes the entire
	// response visible atomically.
	writer.writeRecord({
		key,
		revision,
		...(incarnation === undefined ? {} : { incarnation }),
		fields
	});
	const storedClock = writer.recordClock(key);
	if (
		storedClock === undefined ||
		storedClock.tombstoned ||
		storedClock.revision !== String(revision) ||
		(incarnation !== undefined && storedClock.incarnation !== String(incarnation))
	) {
		// The row was rejected by a newer revision/lifecycle fence. Its identity
		// cannot certify membership for the current incarnation, even if this
		// response carries a newer outer checkpoint.
		return { partial: true };
	}
	for (const relationship of selection.relationships ?? []) {
		const relationshipPath = [...path, relationship.responseKey];
		const hasValue = Object.prototype.hasOwnProperty.call(value, relationship.responseKey);
		const blocked = pathBlocked(errors, relationshipPath);
		const hasErrors = pathHasErrors(errors, relationshipPath);
		const rawValue = hasValue ? value[relationship.responseKey] : undefined;
		const argumentsValue = resolveArguments(relationship.arguments, variables);
		const indexKey = replicaIndexKey({
			parent: key,
			field: relationship.field,
			arguments: argumentsValue
		});
		if (!hasValue || blocked || (rawValue === null && hasErrors)) {
			writer.markIndexStale(
				indexKey,
				hasErrors ? 'graphql-partial-error' : 'incomplete-result',
				snapshotRevision
			);
			partial = true;
			continue;
		}
		const branch = normalizeBranch(
			writer,
			relationship,
			rawValue,
			relationshipPath,
			snapshotRevision,
			variables,
			errors
		);
		const relationshipPartial = branch.partial || hasErrors;
		// The exact parent+field+arguments index is authoritative. A single link
		// slot on the parent cannot represent two aliases/operations with different
		// arguments and cannot safely share the entity row's revision clock.
		writer.writeIndex({
			key: indexKey,
			revision: snapshotRevision,
			records: branch.keys,
			complete: !relationshipPartial,
			metadata: indexMetadata(
				relationship,
				argumentsValue,
				branch,
				relationshipPartial,
				hasErrors,
				key,
				revision
			)
		});
		partial ||= relationshipPartial;
	}

	return { key, partial };
}

function indexMetadata(
	selection: ReplicaRootSelection | ReplicaRelationshipSelection,
	argumentsValue: Readonly<Record<string, CacheValue>>,
	branch: NormalizedBranch,
	partial: boolean,
	hasErrors: boolean,
	parent?: RecordKey,
	parentRevision?: Revision
): CacheIndexMetadata {
	return Object.freeze({
		...(parent === undefined ? {} : { parent }),
		...(parentRevision === undefined ? {} : { parentRevision: String(parentRevision) }),
		field: selection.field,
		arguments: argumentsValue,
		coverage: coverageFromArtifact(selection.coverage, argumentsValue, branch.keys.length),
		dependencies: Object.freeze([...new Set(selection.dependencies)].sort()),
		...(partial
			? { staleReason: hasErrors ? 'graphql-partial-error' : 'incomplete-result' }
			: {}),
		nullValue: branch.nullValue
	});
}

function responseRevision(
	value: Readonly<Record<string, unknown>>,
	responseKey: string | undefined,
	fallback: Revision | undefined,
	description: string
): Revision | undefined {
	if (responseKey === undefined) return fallback;
	if (!Object.prototype.hasOwnProperty.call(value, responseKey)) {
		throw new TypeError(`${description} field ${responseKey} is missing`);
	}
	const revision = value[responseKey];
	if (
		typeof revision !== 'string' &&
		typeof revision !== 'number' &&
		typeof revision !== 'bigint'
	) {
		throw new TypeError(`${description} must be a string, number, or bigint`);
	}
	return revision;
}

function collectErrorPaths(errors: readonly GqlError[]): ErrorPaths {
	const paths: Array<readonly (string | number)[]> = [];
	let global = false;
	for (const error of errors) {
		if (!Array.isArray(error.path) || error.path.length === 0) {
			global = true;
			continue;
		}
		paths.push(Object.freeze([...error.path]));
	}
	return { global, paths };
}

function pathBlocked(errors: ErrorPaths, path: readonly (string | number)[]): boolean {
	return errors.global || errors.paths.some((errorPath) => isPrefix(errorPath, path));
}

function pathHasErrors(errors: ErrorPaths, path: readonly (string | number)[]): boolean {
	return (
		errors.global ||
		errors.paths.some(
			(errorPath) => isPrefix(path, errorPath) || isPrefix(errorPath, path)
		)
	);
}

function isPrefix(
	prefix: readonly (string | number)[],
	value: readonly (string | number)[]
): boolean {
	return prefix.length <= value.length && prefix.every((entry, index) => entry === value[index]);
}

function validateArtifact(artifact: { readonly id: string; readonly roots: readonly unknown[] }): void {
	if (!artifact || typeof artifact !== 'object') throw new TypeError('replica artifact is required');
	if (typeof artifact.id !== 'string' || artifact.id.length === 0) {
		throw new TypeError('replica artifact id must be a non-empty string');
	}
	if (!Array.isArray(artifact.roots) || artifact.roots.length === 0) {
		throw new TypeError(`operation ${artifact.id} must contain at least one root selection`);
	}
}

function isObject(value: unknown): value is Readonly<Record<string, unknown>> {
	return value !== null && typeof value === 'object' && !Array.isArray(value);
}

function deepEqual(left: unknown, right: unknown): boolean {
	if (Object.is(left, right)) return true;
	if (typeof left !== typeof right || left === null || right === null) return false;
	if (typeof left !== 'object' || typeof right !== 'object') return false;
	if (Array.isArray(left) || Array.isArray(right)) {
		if (!Array.isArray(left) || !Array.isArray(right) || left.length !== right.length) return false;
		return left.every((entry, index) => deepEqual(entry, right[index]));
	}
	const leftRecord = left as Readonly<Record<string, unknown>>;
	const rightRecord = right as Readonly<Record<string, unknown>>;
	const leftKeys = Object.keys(leftRecord);
	const rightKeys = Object.keys(rightRecord);
	return (
		leftKeys.length === rightKeys.length &&
		leftKeys.every(
			(key) =>
				Object.prototype.hasOwnProperty.call(rightRecord, key) &&
				deepEqual(leftRecord[key], rightRecord[key])
		)
	);
}
