import type { ReplicaValue } from '../types.js';
import { isPlainRecord } from '../../lib/is-plain-record.js';
import {
	GRAPHQL_NAME
} from './constants.js';
import type {
	ReplicaCommandConfirmations,
	ReplicaCommandDirectProjection,
	ReplicaCommandEffect,
	ReplicaCommandEffectExpression,
	ReplicaCommandEffectField,
	ReplicaCommandEffectKey,
	ReplicaCommandGenerators,
	ReplicaCommandInputDefault,
	ReplicaCommandShape,
	ReplicaCommandTypeDefinition,
	ReplicaCommandTypeField,
	ReplicaMatchedTrustedPresetInventory,
	ReplicaPreparedCommand,
	ReplicaPreparedCommandEffect,
	ReplicaPreparedConfirmations,
	ReplicaPreparedEffectField,
	ReplicaPreparedEffectKey
} from './types.js';
import {
	artifactInvalid,
	inputInvalid,
	requiredString
} from './util.js';
import {
	cloneDefinitionValue,
	cloneJson,
	cloneRelationship
} from './clone.js';

export function materializeInput(
	shape: ReplicaCommandShape,
	input: unknown,
	defaults: readonly ReplicaCommandInputDefault[],
	generators: ReplicaCommandGenerators | undefined
): unknown {
	switch (shape.kind) {
		case 'none':
			if (input !== undefined) inputInvalid('input');
			return undefined;
		case 'object':
			return cloneDefinitionValue(
				shape.definition,
				input,
				[],
				defaults,
				generators,
				'input'
			);
	}
}

export function resolveEffect(
	effect: ReplicaCommandEffect,
	input: unknown,
	path: string,
	trustedPresets: ReplicaMatchedTrustedPresetInventory | undefined
): ReplicaPreparedCommandEffect {
	switch (effect.kind) {
		case 'upsert':
		case 'patch':
			return Object.freeze({
				kind: effect.kind,
				model: requiredString(effect.model, `${path}.model`),
				key: resolveKey(effect.key, input, `${path}.key`, trustedPresets),
				fields: resolveFields(
					effect.fields,
					input,
					`${path}.fields`,
					trustedPresets
				)
			});
		case 'delete':
			return Object.freeze({
				kind: effect.kind,
				model: requiredString(effect.model, `${path}.model`),
				key: resolveKey(effect.key, input, `${path}.key`, trustedPresets)
			});
		case 'link':
		case 'unlink':
			return Object.freeze({
				kind: effect.kind,
				relationship: cloneRelationship(
					effect.relationship,
					`${path}.relationship`
				),
				source: resolveKey(
					effect.source,
					input,
					`${path}.source`,
					trustedPresets
				),
				target: resolveKey(
					effect.target,
					input,
					`${path}.target`,
					trustedPresets
				)
			});
		case 'invalidate_model':
			return Object.freeze({
				kind: effect.kind,
				model: requiredString(effect.model, `${path}.model`)
			});
		case 'invalidate_relationship':
			return Object.freeze({
				kind: effect.kind,
				relationship: cloneRelationship(
					effect.relationship,
					`${path}.relationship`
				),
				source: resolveKey(
					effect.source,
					input,
					`${path}.source`,
					trustedPresets
				)
			});
		default:
			artifactInvalid(`${path}.kind`);
	}
}

export function resolveKey(
	key: ReplicaCommandEffectKey,
	input: unknown,
	path: string,
	trustedPresets: ReplicaMatchedTrustedPresetInventory | undefined
): ReplicaPreparedEffectKey {
	if (
		key === null ||
		typeof key !== 'object' ||
		!Array.isArray(key.fields) ||
		key.fields.length === 0
	) {
		artifactInvalid(path);
	}
	return Object.freeze({
		fields: resolveFields(
			key.fields,
			input,
			`${path}.fields`,
			trustedPresets
		)
	});
}

export function resolveFields(
	fields: readonly ReplicaCommandEffectField[],
	input: unknown,
	path: string,
	trustedPresets: ReplicaMatchedTrustedPresetInventory | undefined
): readonly ReplicaPreparedEffectField[] {
	if (!Array.isArray(fields)) artifactInvalid(path);
	const names = new Set<string>();
	return Object.freeze(
		fields.map((field, index) => {
			const fieldPath = `${path}[${index}]`;
			const name = requiredString(field.field, `${fieldPath}.field`);
			if (names.has(name)) artifactInvalid(`${fieldPath}.field`);
			names.add(name);
			return Object.freeze({
				field: name,
				value: resolveExpression(
					field.value,
					input,
					`${fieldPath}.value`,
					trustedPresets
				)
			});
		})
	);
}

export function resolveExpression(
	expression: ReplicaCommandEffectExpression,
	input: unknown,
	path: string,
	trustedPresets: ReplicaMatchedTrustedPresetInventory | undefined
): ReplicaValue {
	switch (expression.kind) {
		case 'input':
			return resolveInputPath(input, expression.path, path);
		case 'constant':
			return cloneJson(expression.value, `${path}.value`);
		case 'null':
			return null;
		case 'trusted_preset':
			if (trustedPresets === undefined) artifactInvalid(path);
			return trustedPresets.resolve(expression.name);
		default:
			artifactInvalid(`${path}.kind`);
	}
}

export function resolveInputPath(
	input: unknown,
	segments: readonly string[],
	path: string
): ReplicaValue {
	if (
		!Array.isArray(segments) ||
		segments.length === 0 ||
		segments.some((segment) => !GRAPHQL_NAME.test(segment))
	) {
		artifactInvalid(`${path}.path`);
	}
	let current = input;
	for (const segment of segments) {
		if (
			!isPlainRecord(current) ||
			!Object.prototype.hasOwnProperty.call(current, segment)
		) {
			inputInvalid(`input.${segments.join('.')}`);
		}
		current = current[segment];
	}
	if (current === undefined) inputInvalid(`input.${segments.join('.')}`);
	return current as ReplicaValue;
}

export function resolveConfirmations(
	confirmations: ReplicaCommandConfirmations | undefined,
	input: unknown,
	trustedPresets: ReplicaMatchedTrustedPresetInventory | undefined
): ReplicaPreparedConfirmations | undefined {
	if (confirmations === undefined) return undefined;
	if (confirmations.kind === 'unavailable') {
		return Object.freeze({ kind: 'unavailable' as const });
	}
	return Object.freeze({
		kind: 'finite' as const,
		expected: Object.freeze(
			confirmations.expected.map((confirmation, index) => {
				const path = `artifact.confirmations.expected[${index}]`;
				const partition =
					confirmation.partition === undefined
						? undefined
						: resolveExpression(
								confirmation.partition,
								input,
								`${path}.partition`,
								trustedPresets
							);
				return Object.freeze({
					projector: requiredString(
						confirmation.projector,
						`${path}.projector`
					),
					model: requiredString(confirmation.model, `${path}.model`),
					key: resolveKey(
						confirmation.key,
						input,
						`${path}.key`,
						trustedPresets
					),
					...(partition === undefined ? {} : { partition })
				});
			})
		)
	});
}

export function resolveDirectProjection(
	direct: ReplicaCommandDirectProjection | undefined,
	input: unknown,
	trustedPresets: ReplicaMatchedTrustedPresetInventory | undefined
): ReplicaPreparedCommand<unknown>['directProjection'] | undefined {
	if (direct === undefined) return undefined;
	const partition =
		direct.partition === undefined
			? undefined
			: resolveExpression(
					direct.partition,
					input,
					'artifact.directProjection.partition',
					trustedPresets
				);
	return Object.freeze({
		topology: Object.freeze({ ...direct.topology }),
		model: direct.model,
		identityFields: Object.freeze([...direct.identityFields]),
		...(partition === undefined ? {} : { partition }),
		changeEpoch: direct.changeEpoch
	});
}

export function fieldAtPath(
	definition: ReplicaCommandTypeDefinition,
	segments: readonly string[],
	path: string
): ReplicaCommandTypeField {
	let current = definition;
	for (let index = 0; index < segments.length; index += 1) {
		const field = current.fields.find(({ name }) => name === segments[index]);
		if (field === undefined) artifactInvalid(`${path}.path`);
		if (index + 1 === segments.length) return field;
		if (field.list || field.nested === undefined) {
			artifactInvalid(`${path}.path`);
		}
		current = field.nested;
	}
	artifactInvalid(`${path}.path`);
}

