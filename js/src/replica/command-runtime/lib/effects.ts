import {
	type ReplicaPreparedCommand,
	type ReplicaPreparedCommandEffect,
	type ReplicaPreparedEffectKey
} from '../../commands.js';
import { replicaRecordKey } from '../../identity.js';
import type { ReplicaIndexSemanticChange } from '../../index-maintenance.js';
import type {
	ReplicaModelArtifact,
	ReplicaOptimisticWriter,
	ReplicaValue
} from '../../types.js';

export function applyOptimisticEffects(
	writer: ReplicaOptimisticWriter,
	effects: readonly ReplicaPreparedCommandEffect[]
): void {
	for (const effect of effects) {
		switch (effect.kind) {
			case 'upsert':
			case 'patch': {
				const model = modelFromKey(effect.model, effect.key);
				writer.writeRecord(model, identityFromKey(effect.key), {
					fields: fieldsFromEffect(effect.model, effect.key, effect.fields)
				});
				break;
			}
			case 'delete':
				writer.tombstoneRecord(
					modelFromKey(effect.model, effect.key),
					identityFromKey(effect.key)
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
					effect.relationship.sourceModel,
					effect.source
				);
				const target = modelFromKey(
					effect.relationship.targetModel,
					effect.target
				);
				changes.push(
					Object.freeze({
						kind: effect.kind,
						sourceModel: effect.relationship.sourceModel,
						field: effect.relationship.field,
						targetModel: effect.relationship.targetModel,
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

export function modelFromKey(
	model: string,
	key: ReplicaPreparedEffectKey
): ReplicaModelArtifact {
	return Object.freeze({
		id: model,
		identityFields: Object.freeze(key.fields.map(({ field }) => field))
	});
}

export function identityFromKey(
	key: ReplicaPreparedEffectKey
): readonly ReplicaValue[] {
	return Object.freeze(key.fields.map(({ value }) => value));
}

export function fieldsFromEffect(
	model: string,
	key: ReplicaPreparedEffectKey,
	fields: readonly { readonly field: string; readonly value: ReplicaValue }[]
): Readonly<Record<string, ReplicaValue>> {
	const result: Record<string, ReplicaValue> = Object.create(null) as Record<
		string,
		ReplicaValue
	>;
	for (const field of [
		{ field: '__typename', value: model },
		...key.fields,
		...fields
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

