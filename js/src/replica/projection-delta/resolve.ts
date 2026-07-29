import type { DistributedTrustedPreset } from '../../protocol.js';
import type { ReplicaValue } from '../types.js';
import type {
	PreparedCommandProjection,
	PreparedProjectionOperation,
	PreparedProjectionScope,
	ProjectionDeltaField,
	ProjectionDeltaScope,
	ProjectionDeltaValue,
	ProjectionPreviewMutation,
	ProjectionPreviewScope,
	ProjectionPreviewValue,
	ReplicaCommandProjection
} from './types.js';

const missing = Symbol('projection-preview-missing');
const emptyUnsetFields: readonly string[] = Object.freeze([]);

export function prepareCommandProjection(
	contract: ReplicaCommandProjection,
	input: unknown,
	trustedPresets: readonly DistributedTrustedPreset[]
): PreparedCommandProjection {
	const presets = new Map(trustedPresets.map((preset) => [preset.name, preset]));
	try {
		const preview = contract.preview.operations.map(({ mutation }) =>
			resolvePreviewMutation(mutation, input, presets)
		);
		return Object.freeze({
			contract,
			preview: Object.freeze(preview),
			revalidate: contract.preview.recoveries.length !== 0
		});
	} catch {
		// Preview is only a convenience. Missing client authority must never be
		// promoted into invented cache truth.
		return Object.freeze({
			contract,
			preview: Object.freeze([]),
			revalidate: true
		});
	}
}

export function operationsFromProjectionDelta(
	scopes: readonly {
		readonly mutation:
			| import('./types.js').ProjectionDeltaMutation;
	}[]
): readonly PreparedProjectionOperation[] {
	return Object.freeze(
		scopes.map(({ mutation }) => {
			switch (mutation.op) {
				case 'upsert':
					return Object.freeze({
						kind: 'upsert' as const,
						scope: actualScope(mutation.scope),
						fields: fieldsFromDelta(mutation.fields),
						replace: mutation.replace
					});
				case 'patch':
					return Object.freeze({
						kind: 'patch' as const,
						scope: actualScope(mutation.scope),
						fields: fieldsFromDelta(mutation.set),
						unset: mutation.unset,
						ifPresent: true as const
					});
				case 'delete':
					return Object.freeze({
						kind: 'delete' as const,
						scope: actualScope(mutation.scope)
					});
				case 'link':
				case 'unlink':
					return Object.freeze({
						kind: mutation.op,
						relationship: mutation.relationship,
						source: actualScope(mutation.source),
						target: actualScope(mutation.target)
					});
				case 'invalidate_model':
					return Object.freeze({
						kind: 'invalidate_model' as const,
						model: mutation.model
					});
				case 'invalidate_relationship':
					return Object.freeze({
						kind: 'invalidate_relationship' as const,
						relationship: mutation.relationship,
						source: actualScope(mutation.source)
					});
			}
		})
	);
}

function resolvePreviewMutation(
	mutation: ProjectionPreviewMutation,
	input: unknown,
	presets: ReadonlyMap<string, DistributedTrustedPreset>
): PreparedProjectionOperation {
	switch (mutation.op) {
		case 'upsert':
			return Object.freeze({
				kind: 'upsert' as const,
				scope: previewScope(mutation.scope, input, presets),
				fields: previewFields(mutation.fields, input, presets),
				replace: mutation.replace
			});
		case 'patch':
			return Object.freeze({
				kind: 'patch' as const,
				scope: previewScope(mutation.scope, input, presets),
				fields: previewFields(mutation.set, input, presets),
				unset: mutation.unset ?? emptyUnsetFields,
				ifPresent: true as const
			});
		case 'delete':
			return Object.freeze({
				kind: 'delete' as const,
				scope: previewScope(mutation.scope, input, presets)
			});
		case 'link':
		case 'unlink':
			return Object.freeze({
				kind: mutation.op,
				relationship: mutation.relationship,
				source: previewScope(mutation.source, input, presets),
				target: previewScope(mutation.target, input, presets)
			});
		case 'invalidate_model':
			if (mutation.partition?.kind === 'expression') {
				requireValue(
					resolvePreviewValue(mutation.partition.expression, input, presets)
				);
			}
			return Object.freeze({
				kind: 'invalidate_model' as const,
				model: mutation.model
			});
		case 'invalidate_relationship':
			return Object.freeze({
				kind: 'invalidate_relationship' as const,
				relationship: mutation.relationship,
				source: previewScope(mutation.source, input, presets)
			});
	}
}

function previewScope(
	scope: ProjectionPreviewScope,
	input: unknown,
	presets: ReadonlyMap<string, DistributedTrustedPreset>
): PreparedProjectionScope {
	if (scope.partition.kind === 'expression') {
		requireValue(resolvePreviewValue(scope.partition.expression, input, presets));
	}
	return Object.freeze({
		model: scope.model,
		key: Object.freeze(
			scope.key.map(({ field, value }) =>
				Object.freeze({
					field,
					value: requireScalar(
						requireValue(resolvePreviewValue(value, input, presets))
					)
				})
			)
		)
	});
}

function actualScope(scope: ProjectionDeltaScope): PreparedProjectionScope {
	return Object.freeze({
		model: scope.model,
		key: Object.freeze(
			scope.key.map(({ field, value }) =>
				Object.freeze({ field, value: requireScalar(deltaValue(value)) })
			)
		)
	});
}

function previewFields(
	fields: readonly { readonly field: string; readonly value: ProjectionPreviewValue }[],
	input: unknown,
	presets: ReadonlyMap<string, DistributedTrustedPreset>
): Readonly<Record<string, ReplicaValue>> {
	return Object.freeze(
		Object.fromEntries(
			fields.map(({ field, value }) => [
				field,
				requireValue(resolvePreviewValue(value, input, presets))
			])
		)
	);
}

function fieldsFromDelta(
	fields: readonly ProjectionDeltaField[]
): Readonly<Record<string, ReplicaValue>> {
	return Object.freeze(
		Object.fromEntries(fields.map(({ field, value }) => [field, deltaValue(value)]))
	);
}

function resolvePreviewValue(
	expression: ProjectionPreviewValue,
	input: unknown,
	presets: ReadonlyMap<string, DistributedTrustedPreset>
): ReplicaValue | typeof missing {
	switch (expression.kind) {
		case 'input':
		case 'generated_default':
			return valueAtPath(input, expression.path);
		case 'trusted_preset': {
			const preset = presets.get(expression.name);
			if (preset === undefined || preset.codec !== expression.codec) return missing;
			return preset.value as ReplicaValue;
		}
		case 'constant':
			return deltaValue(expression.value);
		case 'null':
			return null;
		case 'list': {
			const values = expression.values.map((value) =>
				resolvePreviewValue(value, input, presets)
			);
			return values.some((value) => value === missing)
				? missing
				: Object.freeze(values as ReplicaValue[]);
		}
		case 'object': {
			const fields: [string, ReplicaValue][] = [];
			for (const field of expression.fields) {
				const value = resolvePreviewValue(field.value, input, presets);
				if (value === missing) return missing;
				fields.push([field.name, value]);
			}
			return Object.freeze(Object.fromEntries(fields));
		}
		case 'transform':
			if (expression.transform === 'first_present') {
				for (const argument of expression.arguments) {
					const value = resolvePreviewValue(argument, input, presets);
					if (value !== missing) return value;
				}
				return missing;
			}
			return expression.arguments
				.map((argument) =>
					requireValue(resolvePreviewValue(argument, input, presets))
				)
				.map((value) => {
					if (typeof value !== 'string') throw new TypeError('projection');
					return value;
				})
				.join('');
	}
}

function valueAtPath(value: unknown, path: readonly string[]): ReplicaValue | typeof missing {
	let current = value;
	for (const segment of path) {
		if (
			current === null ||
			typeof current !== 'object' ||
			!Object.hasOwn(current, segment)
		) return missing;
		current = (current as Record<string, unknown>)[segment];
	}
	return isReplicaValue(current) ? current : missing;
}

export function deltaValue(value: ProjectionDeltaValue): ReplicaValue {
	switch (value.type) {
		case 'null':
			return null;
		case 'boolean':
			return value.value;
		case 'string':
			return value.value;
		case 'enum':
			return value.value.variant;
		case 'i64':
		case 'u64': {
			const parsed = BigInt(value.value);
			return parsed >= BigInt(Number.MIN_SAFE_INTEGER) &&
				parsed <= BigInt(Number.MAX_SAFE_INTEGER)
				? Number(parsed)
				: value.value;
		}
		case 'f64':
			return Number(value.value);
		case 'list':
			return Object.freeze(value.value.map(deltaValue));
		case 'object':
			return Object.freeze(
				Object.fromEntries(
					value.value.map((field) => [field.field, deltaValue(field.value)])
				)
			);
	}
}

function requireValue(value: ReplicaValue | typeof missing): ReplicaValue {
	if (value === missing) throw new TypeError('projection preview is unavailable');
	return value;
}

function requireScalar(value: ReplicaValue): ReplicaValue {
	if (Array.isArray(value) || (value !== null && typeof value === 'object')) {
		throw new TypeError('projection identity must be scalar');
	}
	return value;
}

function isReplicaValue(value: unknown, depth = 0): value is ReplicaValue {
	if (depth > 64) return false;
	if (
		value === null ||
		typeof value === 'string' ||
		typeof value === 'boolean' ||
		(typeof value === 'number' && Number.isFinite(value))
	) return true;
	if (Array.isArray(value)) return value.every((item) => isReplicaValue(item, depth + 1));
	if (typeof value !== 'object') return false;
	return Object.entries(value).every(
		([, item]) => isReplicaValue(item, depth + 1)
	);
}
