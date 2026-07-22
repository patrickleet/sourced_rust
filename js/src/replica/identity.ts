import { cacheIndexKey } from '../internal/cache-engine.js';
import type { GraphqlVariables } from '../types.js';
import type {
	ReplicaArgumentsArtifact,
	ReplicaCoverageArtifact,
	ReplicaIdentity,
	ReplicaIndexCoverage,
	ReplicaModelArtifact,
	ReplicaValue
} from './types.js';

export function replicaRecordKey(
	model: ReplicaModelArtifact,
	identity: ReplicaIdentity
): string {
	assertName(model.id, 'model id');
	if (!Array.isArray(model.identityFields) || model.identityFields.length === 0) {
		throw new TypeError(`model ${model.id} must declare at least one identity field`);
	}
	const parts = Array.isArray(identity) ? identity : [identity];
	if (parts.length !== model.identityFields.length) {
		throw new TypeError(
			`model ${model.id} identity expected ${model.identityFields.length} part(s), received ${parts.length}`
		);
	}
	return `record:${encodeURIComponent(model.id)}:${canonicalCacheValue(
		parts.map((part) => cloneJsonValue(part))
	)}`;
}

export function replicaIndexKey(input: {
	readonly parent?: string;
	readonly field: string;
	readonly arguments?: Readonly<Record<string, ReplicaValue>>;
}): string {
	return cacheIndexKey({
		...(input.parent === undefined ? {} : { parent: input.parent }),
		field: input.field,
		arguments: input.arguments ?? {}
	});
}

export function resolveArguments(
	artifact: ReplicaArgumentsArtifact | undefined,
	variables: GraphqlVariables
): Readonly<Record<string, ReplicaValue>> {
	if (artifact === undefined) return Object.freeze({});
	const entries: Array<readonly [string, ReplicaValue]> = [];
	for (const [argument, source] of Object.entries(artifact)) {
		assertName(argument, 'GraphQL argument');
		if (source.kind === 'literal') {
			entries.push([argument, cloneJsonValue(source.value)]);
			continue;
		}
		if (source.kind !== 'variable') throw new TypeError('unsupported argument source');
		assertName(source.name, 'GraphQL variable');
		if (!Object.prototype.hasOwnProperty.call(variables, source.name)) continue;
		const value = variables[source.name];
		if (value === undefined) continue;
		entries.push([argument, cloneJsonValue(value)]);
	}
	return freezeRecord(entries);
}

export function canonicalVariables(variables: GraphqlVariables): string {
	return canonicalCacheValue(cloneJsonObject(variables));
}

export function coverageFromArtifact(
	artifact: ReplicaCoverageArtifact | undefined,
	argumentsValue: Readonly<Record<string, ReplicaValue>>,
	returned: number
): ReplicaIndexCoverage {
	if (artifact === undefined || artifact.kind === 'complete') {
		return Object.freeze({ kind: 'complete' });
	}
	if (artifact.kind === 'offset') {
		const offset = numericArgument(argumentsValue, artifact.offsetArgument) ?? 0;
		const limit = numericArgument(argumentsValue, artifact.limitArgument);
		return Object.freeze({
			kind: 'offset' as const,
			offset,
			...(limit === undefined ? {} : { limit }),
			returned
		});
	}
	return Object.freeze({
		kind: 'cursor' as const,
		...cacheArgument(argumentsValue, artifact.afterArgument, 'after'),
		...cacheArgument(argumentsValue, artifact.beforeArgument, 'before'),
		...numericCoverageArgument(argumentsValue, artifact.firstArgument, 'first'),
		...numericCoverageArgument(argumentsValue, artifact.lastArgument, 'last')
	});
}

export function cloneJsonObject(
	value: GraphqlVariables
): Readonly<Record<string, ReplicaValue>> {
	const entries: Array<readonly [string, ReplicaValue]> = [];
	for (const [key, entry] of Object.entries(value)) {
		if (entry === undefined) continue;
		entries.push([key, cloneJsonValue(entry)]);
	}
	return freezeRecord(entries);
}

export function cloneJsonValue(value: unknown, ancestors = new Set<object>()): ReplicaValue {
	if (
		value === null ||
		typeof value === 'string' ||
		typeof value === 'boolean' ||
		typeof value === 'number'
	) {
		if (typeof value === 'number' && !Number.isFinite(value)) {
			throw new TypeError('replica values must contain only finite numbers');
		}
		return value;
	}
	if (typeof value !== 'object') {
		throw new TypeError('replica values must be JSON-compatible');
	}
	if (ancestors.has(value)) throw new TypeError('replica values must not contain cycles');
	ancestors.add(value);
	let cloned: ReplicaValue;
	if (Array.isArray(value)) {
		cloned = Object.freeze(value.map((entry) => cloneJsonValue(entry, ancestors)));
	} else {
		const prototype = Object.getPrototypeOf(value);
		if (prototype !== Object.prototype && prototype !== null) {
			throw new TypeError('replica objects must be plain JSON objects');
		}
		cloned = freezeRecord(
			Object.entries(value).flatMap(([key, entry]) =>
				entry === undefined ? [] : [[key, cloneJsonValue(entry, ancestors)] as const]
			)
		);
	}
	ancestors.delete(value);
	return cloned;
}

export function canonicalCacheValue(value: ReplicaValue): string {
	if (value === null || typeof value !== 'object') return JSON.stringify(value);
	if (Array.isArray(value)) return `[${value.map(canonicalCacheValue).join(',')}]`;
	const record = value as Readonly<Record<string, ReplicaValue>>;
	return `{${Object.keys(record)
		.sort()
		.map((key) => `${JSON.stringify(key)}:${canonicalCacheValue(record[key]!)}`)
		.join(',')}}`;
}

function cacheArgument<K extends 'after' | 'before'>(
	argumentsValue: Readonly<Record<string, ReplicaValue>>,
	argument: string | undefined,
	key: K
): Partial<Record<K, ReplicaValue>> {
	if (argument === undefined || !Object.prototype.hasOwnProperty.call(argumentsValue, argument)) {
		return {};
	}
	return { [key]: argumentsValue[argument]! } as Partial<Record<K, ReplicaValue>>;
}

function numericCoverageArgument<K extends 'first' | 'last'>(
	argumentsValue: Readonly<Record<string, ReplicaValue>>,
	argument: string | undefined,
	key: K
): Partial<Record<K, number>> {
	const value = numericArgument(argumentsValue, argument);
	return value === undefined ? {} : ({ [key]: value } as Partial<Record<K, number>>);
}

function numericArgument(
	argumentsValue: Readonly<Record<string, ReplicaValue>>,
	argument: string | undefined
): number | undefined {
	if (argument === undefined || !Object.prototype.hasOwnProperty.call(argumentsValue, argument)) {
		return undefined;
	}
	const value = argumentsValue[argument];
	if (!Number.isSafeInteger(value) || (value as number) < 0) {
		throw new TypeError(`pagination argument ${argument} must be a non-negative integer`);
	}
	return value as number;
}

function freezeRecord<T>(
	entries: readonly (readonly [string, T])[]
): Readonly<Record<string, T>> {
	const record: Record<string, T> = {};
	for (const [key, value] of entries) {
		Object.defineProperty(record, key, {
			value,
			enumerable: true,
			configurable: false,
			writable: false
		});
	}
	return Object.freeze(record);
}

function assertName(value: string, description: string): void {
	if (typeof value !== 'string' || value.length === 0) {
		throw new TypeError(`${description} must be a non-empty string`);
	}
}
