import {
	type ReplicaPreparedCommand,
} from '../../commands.js';
import type {
	PreparedProjectionOperation,
	PreparedProjectionScope
} from '../../projection-delta/index.js';
import { replicaRecordKey } from '../../identity.js';
import type { ReplicaIndexSemanticChange } from '../../index-maintenance.js';
import type {
	ReplicaModelArtifact,
	ReplicaOptimisticWriter,
	ReplicaValue
} from '../../types.js';

export function applyOptimisticEffects(
	writer: ReplicaOptimisticWriter,
	effects: readonly PreparedProjectionOperation[]
): void {
	for (const effect of effects) {
		switch (effect.kind) {
			case 'upsert':
			case 'patch': {
				const model = modelFromKey(effect.scope);
				const fields = fieldsFromEffect(effect.scope, effect.fields);
				const unset =
					effect.kind === 'upsert'
						? effect.replace.filter((field) => !Object.hasOwn(effect.fields, field))
						: effect.unset;
				writer.writeRecord(model, identityFromKey(effect.scope), {
					fields,
					...(unset.length === 0 ? {} : { unset }),
					...(effect.kind === 'patch' ? { ifPresent: true } : {})
				});
				break;
			}
			case 'delete':
				writer.tombstoneRecord(
					modelFromKey(effect.scope),
					identityFromKey(effect.scope)
				);
				break;
			case 'link':
			case 'unlink':
			case 'invalidate_model':
			case 'invalidate_relationship':
				// Task 8 consumes the exact semantic context. Guessing a to-one
				// record link for a to-many relationship would corrupt truth.
				break;
		}
	}
}

export function preparedSemanticChanges<TInput, TOutput>(
	prepared: ReplicaPreparedCommand<TInput, TOutput>
): readonly ReplicaIndexSemanticChange[] {
	const dependencies = Object.freeze([...prepared.revalidation.dependencies]);
	const changes: ReplicaIndexSemanticChange[] = [];
	for (const effect of prepared.optimistic.operations) {
		switch (effect.kind) {
			case 'upsert':
			case 'patch':
			case 'delete':
				// DistributedReplica captures ordinary writer mutations into the
				// same layer context. Supplying them again would double-apply the
				// semantic record operation.
				break;
			case 'link':
			case 'unlink': {
				const source = modelFromKey(
					effect.source
				);
				const target = modelFromKey(
					effect.target
				);
				changes.push(
					Object.freeze({
						kind: effect.kind,
						sourceModel: effect.source.model,
						field: effect.relationship,
						targetModel: effect.target.model,
						sourceKey: replicaRecordKey(
							source,
							identityFromKey(effect.source)
						),
						targetKey: replicaRecordKey(
							target,
							identityFromKey(effect.target)
						),
						dependencies
					})
				);
				break;
			}
			case 'invalidate_model':
			case 'invalidate_relationship':
				changes.push(
					Object.freeze({
						kind: 'invalidate',
						dependencies
					})
				);
				break;
		}
	}
	return Object.freeze(changes);
}

/**
 * Exact record identities whose aggregate-facing command dispatches must retain
 * invocation order. Optimistic layers still apply immediately; only transport
 * waits for an earlier command touching the same modeled record to settle.
 */
export function preparedDispatchKeys<TInput, TOutput>(
	prepared: ReplicaPreparedCommand<TInput, TOutput>
): readonly string[] {
	const keys = new Set<string>();
	const addScope = (scope: PreparedProjectionScope): void => {
		keys.add(
			replicaRecordKey(modelFromKey(scope), identityFromKey(scope))
		);
	};

	for (const effect of prepared.optimistic.operations) {
		switch (effect.kind) {
			case 'upsert':
			case 'patch':
			case 'delete':
				addScope(effect.scope);
				break;
			case 'link':
			case 'unlink':
				addScope(effect.source);
				addScope(effect.target);
				break;
			case 'invalidate_relationship':
				addScope(effect.source);
				break;
			case 'invalidate_model':
				/*
				 * A model invalidation has no compiler-proven aggregate
				 * identity. It cannot safely participate in a record queue.
				 */
				break;
		}
	}

	return Object.freeze([...keys].sort());
}

export function modelFromKey(
	scope: PreparedProjectionScope
): ReplicaModelArtifact {
	return Object.freeze({
		id: scope.model,
		identityFields: Object.freeze(scope.key.map(({ field }) => field))
	});
}

export function identityFromKey(
	key: PreparedProjectionScope
): readonly ReplicaValue[] {
	return Object.freeze(key.key.map(({ value }) => value));
}

export function fieldsFromEffect(
	scope: PreparedProjectionScope,
	fields: Readonly<Record<string, ReplicaValue>>
): Readonly<Record<string, ReplicaValue>> {
	const result: Record<string, ReplicaValue> = Object.create(null) as Record<
		string,
		ReplicaValue
	>;
	for (const field of [
		{ field: '__typename', value: scope.model },
		...scope.key,
		...Object.entries(fields).map(([field, value]) => ({ field, value }))
	]) {
		Object.defineProperty(result, field.field, {
			enumerable: true,
			configurable: false,
			writable: false,
			value: field.value
		});
	}
	return Object.freeze(result);
}
