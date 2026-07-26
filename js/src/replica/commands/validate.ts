import type { ReplicaClientSurface } from '../types.js';
import { isPlainRecord } from '../../lib/is-plain-record.js';
import {
	GRAPHQL_NAME,
	MAX_TYPE_DEPTH
} from './constants.js';
import type {
	ReplicaCommandArtifact,
	ReplicaCommandEffect,
	ReplicaCommandEffectExpression,
	ReplicaCommandEffectField,
	ReplicaCommandEffectKey,
	ReplicaCommandEffectRelationship,
	ReplicaCommandInputDefaults,
	ReplicaCommandShape,
	ReplicaCommandTypeDefinition,
	ReplicaTrustedPresetDescriptor,
	TrustedPresetReference
} from './types.js';
import {
	artifactInvalid,
	hash,
	isSupportedCodec,
	nonempty,
	requiredString
} from './util.js';
import {
	parseReplicaTrustedPresetDescriptors,
	trustedPresetName
} from './presets.js';
import { fieldAtPath } from './resolve.js';

export function validateArtifact<TInput, TOutput>(
	artifact: ReplicaCommandArtifact<TInput, TOutput>,
	allowTrustedPresets: boolean
): readonly ReplicaTrustedPresetDescriptor[] {
	if (artifact.version !== 1) artifactInvalid('artifact.version');
	nonempty(artifact.name, 'artifact.name');
	if (!GRAPHQL_NAME.test(artifact.mutationField)) {
		artifactInvalid('artifact.mutationField');
	}
	nonempty(artifact.document, 'artifact.document');
	hash(artifact.operationHash, 'artifact.operationHash');
	if (artifact.protocol.version !== 2) artifactInvalid('artifact.protocol.version');
	hash(artifact.protocol.schemaHash, 'artifact.protocol.schemaHash');
	hash(artifact.protocol.protocolHash, 'artifact.protocol.protocolHash');
	validateClientSurface(artifact.protocol.surface, 'artifact.protocol.surface');
	if (artifact.protocol.operation !== artifact.operationHash) {
		artifactInvalid('artifact.protocol.operation');
	}
	const surfaceTrustedPresets = parseReplicaTrustedPresetDescriptors(
		artifact.protocol.trustedPresets,
		'artifact.protocol.trustedPresets'
	);
	const trustedPresets = parseReplicaTrustedPresetDescriptors(
		artifact.trustedPresets ?? [],
		'artifact.trustedPresets'
	);
	const surfaceByName = new Map(
		surfaceTrustedPresets.map((descriptor) => [
			descriptor.name,
			descriptor.codec
		] as const)
	);
	for (let index = 0; index < trustedPresets.length; index += 1) {
		const descriptor = trustedPresets[index]!;
		if (surfaceByName.get(descriptor.name) !== descriptor.codec) {
			artifactInvalid(`artifact.trustedPresets[${index}]`);
		}
	}
	validateShape(artifact.input, 'artifact.input', 0, new Set());
	validateShape(artifact.output, 'artifact.output', 0, new Set());
	if (artifact.output.kind === 'none') artifactInvalid('artifact.output.kind');
	validateDefaults(artifact.input, artifact.inputDefaults);
	if (
		artifact.consistency !== 'accepted' &&
		artifact.consistency !== 'fact' &&
		artifact.consistency !== 'projected'
	) {
		artifactInvalid('artifact.consistency');
	}
	if (artifact.effects.version !== 1) artifactInvalid('artifact.effects.version');
	if (artifact.effects.fallback !== 'revalidate') {
		artifactInvalid('artifact.effects.fallback');
	}
	if (!Array.isArray(artifact.effects.operations)) {
		artifactInvalid('artifact.effects.operations');
	}
	for (let index = 0; index < artifact.effects.operations.length; index += 1) {
		validateEffectArtifact(
			artifact.effects.operations[index]!,
			artifact.input,
			`artifact.effects.operations[${index}]`
		);
	}
	validateConfirmations(artifact);
	validateDirectProjection(artifact);
	validateRevalidation(artifact);
	validateTrustedPresetReferences(
		artifact,
		trustedPresets,
		allowTrustedPresets
	);
	return trustedPresets;
}

export function validateClientSurface(
	surface: ReplicaClientSurface,
	path: string
): void {
	if (surface === null || typeof surface !== 'object') artifactInvalid(path);
	requiredString(surface.name, `${path}.name`);
	if (surface.kind === 'role') return;
	if (
		surface.kind !== 'application' ||
		!Array.isArray(surface.roles) ||
		surface.roles.length === 0
	) {
		artifactInvalid(path);
	}
	const roles = new Set<string>();
	for (let index = 0; index < surface.roles.length; index += 1) {
		const role = requiredString(surface.roles[index], `${path}.roles[${index}]`);
		if (roles.has(role)) artifactInvalid(`${path}.roles[${index}]`);
		roles.add(role);
	}
}

export function validateTrustedPresetReferences<TInput, TOutput>(
	artifact: ReplicaCommandArtifact<TInput, TOutput>,
	descriptors: readonly ReplicaTrustedPresetDescriptor[],
	allowTrustedPresets: boolean
): void {
	const references = collectTrustedPresetReferences(artifact);
	const declared = new Set(descriptors.map(({ name }) => name));
	for (const reference of references) {
		if (!declared.has(reference.name)) artifactInvalid(reference.path);
	}
	const referenced = new Set(references.map(({ name }) => name));
	for (let index = 0; index < descriptors.length; index += 1) {
		if (!referenced.has(descriptors[index]!.name)) {
			artifactInvalid(`artifact.trustedPresets[${index}]`);
		}
	}
	if (!allowTrustedPresets && references.length !== 0) {
		artifactInvalid(references[0]!.path);
	}
}

export function collectTrustedPresetReferences<TInput, TOutput>(
	artifact: ReplicaCommandArtifact<TInput, TOutput>
): readonly TrustedPresetReference[] {
	const out: TrustedPresetReference[] = [];
	const expression = (
		value: ReplicaCommandEffectExpression,
		path: string
	): void => {
		if (value.kind === 'trusted_preset') {
			out.push(Object.freeze({ name: value.name, path }));
		}
	};
	const fields = (
		value: readonly ReplicaCommandEffectField[],
		path: string
	): void => {
		for (let index = 0; index < value.length; index += 1) {
			expression(value[index]!.value, `${path}[${index}].value`);
		}
	};
	const key = (value: ReplicaCommandEffectKey, path: string): void => {
		fields(value.fields, `${path}.fields`);
	};

	for (let index = 0; index < artifact.effects.operations.length; index += 1) {
		const effect = artifact.effects.operations[index]!;
		const path = `artifact.effects.operations[${index}]`;
		switch (effect.kind) {
			case 'upsert':
			case 'patch':
				key(effect.key, `${path}.key`);
				fields(effect.fields, `${path}.fields`);
				break;
			case 'delete':
				key(effect.key, `${path}.key`);
				break;
			case 'link':
			case 'unlink':
				key(effect.source, `${path}.source`);
				key(effect.target, `${path}.target`);
				break;
			case 'invalidate_relationship':
				key(effect.source, `${path}.source`);
				break;
			case 'invalidate_model':
				break;
		}
	}
	if (artifact.confirmations?.kind === 'finite') {
		for (
			let index = 0;
			index < artifact.confirmations.expected.length;
			index += 1
		) {
			const confirmation = artifact.confirmations.expected[index]!;
			const path = `artifact.confirmations.expected[${index}]`;
			key(confirmation.key, `${path}.key`);
			if (confirmation.partition !== undefined) {
				expression(confirmation.partition, `${path}.partition`);
			}
		}
	}
	if (artifact.directProjection?.partition !== undefined) {
		expression(
			artifact.directProjection.partition,
			'artifact.directProjection.partition'
		);
	}
	return Object.freeze(out);
}

export function validateShape(
	shape: ReplicaCommandShape,
	path: string,
	depth: number,
	active: Set<ReplicaCommandTypeDefinition>
): void {
	if (depth > MAX_TYPE_DEPTH) artifactInvalid(path);
	switch (shape.kind) {
		case 'none':
			return;
		case 'object':
			validateDefinition(shape.definition, `${path}.definition`, depth, active);
			return;
		default:
			artifactInvalid(`${path}.kind`);
	}
}

export function validateDefinition(
	definition: ReplicaCommandTypeDefinition,
	path: string,
	depth: number,
	active: Set<ReplicaCommandTypeDefinition>
): void {
	if (depth > MAX_TYPE_DEPTH || active.has(definition)) artifactInvalid(path);
	if (!GRAPHQL_NAME.test(definition.name) || !Array.isArray(definition.fields)) {
		artifactInvalid(path);
	}
	active.add(definition);
	const names = new Set<string>();
	for (let index = 0; index < definition.fields.length; index += 1) {
		const field = definition.fields[index]!;
		const fieldPath = `${path}.fields[${index}]`;
		if (!GRAPHQL_NAME.test(field.name) || !GRAPHQL_NAME.test(field.typeName)) {
			artifactInvalid(fieldPath);
		}
		if (names.has(field.name)) artifactInvalid(`${fieldPath}.name`);
		names.add(field.name);
		if (
			typeof field.nullable !== 'boolean' ||
			typeof field.list !== 'boolean' ||
			typeof field.itemNullable !== 'boolean' ||
			(field.itemNullable && !field.list) ||
			(field.codec === undefined) === (field.nested === undefined)
		) {
			artifactInvalid(fieldPath);
		}
		if (field.codec !== undefined && !isSupportedCodec(field.codec)) {
			artifactInvalid(`${fieldPath}.codec`);
		}
		if (field.nested !== undefined) {
			if (field.nested.name !== field.typeName) {
				artifactInvalid(`${fieldPath}.nested.name`);
			}
			validateDefinition(field.nested, `${fieldPath}.nested`, depth + 1, active);
		}
	}
	active.delete(definition);
}

export function validateDefaults(
	shape: ReplicaCommandShape,
	defaults: ReplicaCommandInputDefaults | undefined
): void {
	if (defaults === undefined) return;
	if (defaults.version !== 1 || !Array.isArray(defaults.defaults)) {
		artifactInvalid('artifact.inputDefaults');
	}
	if (shape.kind !== 'object' && defaults.defaults.length > 0) {
		artifactInvalid('artifact.inputDefaults.defaults');
	}
	const paths = new Set<string>();
	for (let index = 0; index < defaults.defaults.length; index += 1) {
		const entry = defaults.defaults[index]!;
		const path = `artifact.inputDefaults.defaults[${index}]`;
		if (
			!Array.isArray(entry.path) ||
			entry.path.length === 0 ||
			entry.path.some(
				(segment: unknown) =>
					typeof segment !== 'string' || !GRAPHQL_NAME.test(segment)
			) ||
			(entry.generator !== 'uuid_v7' && entry.generator !== 'ulid')
		) {
			artifactInvalid(path);
		}
		const key = entry.path.join('\u0000');
		if (paths.has(key)) artifactInvalid(`${path}.path`);
		paths.add(key);
		if (shape.kind === 'object') {
			const field = fieldAtPath(shape.definition, entry.path, path);
			if (
				field.nullable ||
				field.list ||
				field.nested !== undefined ||
				field.codec !== 'string' ||
				(field.typeName !== 'ID' && field.typeName !== 'String')
			) {
				artifactInvalid(path);
			}
		}
	}
}

export function validateConfirmations<TInput, TOutput>(
	artifact: ReplicaCommandArtifact<TInput, TOutput>
): void {
	const confirmations = artifact.confirmations;
	if (confirmations === undefined) {
		if (artifact.consistency === 'fact') {
			artifactInvalid('artifact.confirmations');
		}
		return;
	}
	if (
		confirmations.version !== 1 ||
		confirmations.fallback !== 'revalidate' ||
		!Array.isArray(confirmations.expected)
	) {
		artifactInvalid('artifact.confirmations');
	}
	if (confirmations.kind === 'unavailable') {
		if (confirmations.expected.length !== 0) {
			artifactInvalid('artifact.confirmations.expected');
		}
	} else if (confirmations.kind === 'finite') {
		if (confirmations.expected.length === 0) {
			artifactInvalid('artifact.confirmations.expected');
		}
	} else {
		artifactInvalid('artifact.confirmations.kind');
	}
	if (artifact.consistency === 'projected') {
		artifactInvalid('artifact.confirmations');
	}
	if (confirmations.kind === 'finite') {
		for (let index = 0; index < confirmations.expected.length; index += 1) {
			const confirmation = confirmations.expected[index]!;
			const path = `artifact.confirmations.expected[${index}]`;
			requiredString(confirmation.projector, `${path}.projector`);
			requiredString(confirmation.model, `${path}.model`);
			validateKeyArtifact(confirmation.key, artifact.input, `${path}.key`);
			if (confirmation.partition !== undefined) {
				validateExpressionArtifact(
					confirmation.partition,
					artifact.input,
					`${path}.partition`
				);
			}
		}
	}
}

export function validateDirectProjection<TInput, TOutput>(
	artifact: ReplicaCommandArtifact<TInput, TOutput>
): void {
	const direct = artifact.directProjection;
	if (artifact.consistency === 'projected' && direct === undefined) {
		artifactInvalid('artifact.directProjection');
	}
	if (artifact.consistency !== 'projected' && direct !== undefined) {
		artifactInvalid('artifact.directProjection');
	}
	if (direct === undefined) return;
	if (direct.topology.version !== 1) {
		artifactInvalid('artifact.directProjection.topology.version');
	}
	nonempty(direct.topology.name, 'artifact.directProjection.topology.name');
	hash(direct.topology.digest, 'artifact.directProjection.topology.digest');
	nonempty(direct.model, 'artifact.directProjection.model');
	nonempty(direct.changeEpoch, 'artifact.directProjection.changeEpoch');
	if (
		!Array.isArray(direct.identityFields) ||
		direct.identityFields.length === 0 ||
		artifact.output.kind !== 'object'
	) {
		artifactInvalid('artifact.directProjection.identityFields');
	}
	const identityFields = new Set<string>();
	for (let index = 0; index < direct.identityFields.length; index += 1) {
		const fieldName = direct.identityFields[index]!;
		const path = `artifact.directProjection.identityFields[${index}]`;
		if (
			typeof fieldName !== 'string' ||
			!GRAPHQL_NAME.test(fieldName) ||
			identityFields.has(fieldName)
		) {
			artifactInvalid(path);
		}
		identityFields.add(fieldName);
		const outputField = artifact.output.definition.fields.find(
			(field) => field.name === fieldName
		);
		if (
			outputField === undefined ||
			outputField.nullable ||
			outputField.list ||
			outputField.codec === undefined ||
			outputField.nested !== undefined
		) {
			artifactInvalid(path);
		}
	}
	if (direct.partition !== undefined) {
		validateExpressionArtifact(
			direct.partition,
			artifact.input,
			'artifact.directProjection.partition'
		);
	}
}

export function validateRevalidation<TInput, TOutput>(
	artifact: ReplicaCommandArtifact<TInput, TOutput>
): void {
	const plan = artifact.revalidation;
	if (
		plan.version !== 1 ||
		typeof plan.required !== 'boolean' ||
		!Array.isArray(plan.dependencies) ||
		!Array.isArray(plan.models) ||
		!Array.isArray(plan.relationships)
	) {
		artifactInvalid('artifact.revalidation');
	}
	for (const [index, dependency] of plan.dependencies.entries()) {
		nonempty(dependency, `artifact.revalidation.dependencies[${index}]`);
	}
	for (const [index, model] of plan.models.entries()) {
		nonempty(model, `artifact.revalidation.models[${index}]`);
	}
	for (const [index, relationship] of plan.relationships.entries()) {
		validateRelationshipArtifact(
			relationship,
			`artifact.revalidation.relationships[${index}]`
		);
	}
	if (
		(artifact.effects.operations.length === 0 ||
			(artifact.consistency !== 'projected' &&
				artifact.confirmations?.kind !== 'finite')) &&
		!plan.required
	) {
		artifactInvalid('artifact.revalidation.required');
	}
}

export function validateEffectArtifact(
	effect: ReplicaCommandEffect,
	input: ReplicaCommandShape,
	path: string
): void {
	if (effect === null || typeof effect !== 'object') artifactInvalid(path);
	switch (effect.kind) {
		case 'upsert':
		case 'patch':
			requiredString(effect.model, `${path}.model`);
			validateKeyArtifact(effect.key, input, `${path}.key`);
			validateFieldsArtifact(effect.fields, input, `${path}.fields`);
			return;
		case 'delete':
			requiredString(effect.model, `${path}.model`);
			validateKeyArtifact(effect.key, input, `${path}.key`);
			return;
		case 'link':
		case 'unlink':
			validateRelationshipArtifact(
				effect.relationship,
				`${path}.relationship`
			);
			validateKeyArtifact(effect.source, input, `${path}.source`);
			validateKeyArtifact(effect.target, input, `${path}.target`);
			return;
		case 'invalidate_model':
			requiredString(effect.model, `${path}.model`);
			return;
		case 'invalidate_relationship':
			validateRelationshipArtifact(
				effect.relationship,
				`${path}.relationship`
			);
			validateKeyArtifact(effect.source, input, `${path}.source`);
			return;
		default:
			artifactInvalid(`${path}.kind`);
	}
}

export function validateKeyArtifact(
	key: ReplicaCommandEffectKey,
	input: ReplicaCommandShape,
	path: string
): void {
	if (
		key === null ||
		typeof key !== 'object' ||
		!Array.isArray(key.fields) ||
		key.fields.length === 0
	) {
		artifactInvalid(path);
	}
	validateFieldsArtifact(key.fields, input, `${path}.fields`);
}

export function validateFieldsArtifact(
	fields: readonly ReplicaCommandEffectField[],
	input: ReplicaCommandShape,
	path: string
): void {
	if (!Array.isArray(fields)) artifactInvalid(path);
	const names = new Set<string>();
	for (let index = 0; index < fields.length; index += 1) {
		const field = fields[index]!;
		const fieldPath = `${path}[${index}]`;
		const name = requiredString(field.field, `${fieldPath}.field`);
		if (names.has(name)) artifactInvalid(`${fieldPath}.field`);
		names.add(name);
		validateExpressionArtifact(field.value, input, `${fieldPath}.value`);
	}
}

export function validateExpressionArtifact(
	expression: ReplicaCommandEffectExpression,
	input: ReplicaCommandShape,
	path: string
): void {
	if (expression === null || typeof expression !== 'object') {
		artifactInvalid(path);
	}
	switch (expression.kind) {
		case 'input':
			if (
				!Array.isArray(expression.path) ||
				expression.path.length === 0 ||
				expression.path.some(
					(segment: unknown) =>
						typeof segment !== 'string' || !GRAPHQL_NAME.test(segment)
				)
			) {
				artifactInvalid(`${path}.path`);
			}
			if (input.kind !== 'object') artifactInvalid(`${path}.path`);
			fieldAtPath(input.definition, expression.path, path);
			return;
		case 'constant':
			validateArtifactJson(expression.value, `${path}.value`, new Set(), 0);
			return;
		case 'null':
			return;
		case 'trusted_preset':
			trustedPresetName(expression.name, `${path}.name`);
			return;
		case undefined:
		default:
			artifactInvalid(`${path}.kind`);
	}
}

export function validateRelationshipArtifact(
	relationship: ReplicaCommandEffectRelationship,
	path: string
): void {
	if (relationship === null || typeof relationship !== 'object') {
		artifactInvalid(path);
	}
	requiredString(relationship.sourceModel, `${path}.sourceModel`);
	requiredString(relationship.field, `${path}.field`);
	requiredString(relationship.targetModel, `${path}.targetModel`);
}

export function validateArtifactJson(
	value: unknown,
	path: string,
	active: Set<object>,
	depth: number
): void {
	if (depth > MAX_TYPE_DEPTH) artifactInvalid(path);
	if (
		value === null ||
		typeof value === 'string' ||
		typeof value === 'boolean'
	) {
		return;
	}
	if (typeof value === 'number') {
		if (!Number.isFinite(value)) artifactInvalid(path);
		return;
	}
	if (typeof value !== 'object') artifactInvalid(path);
	if (active.has(value)) artifactInvalid(path);
	active.add(value);
	if (Array.isArray(value)) {
		for (let index = 0; index < value.length; index += 1) {
			validateArtifactJson(value[index], `${path}[${index}]`, active, depth + 1);
		}
		active.delete(value);
		return;
	}
	if (!isPlainRecord(value)) artifactInvalid(path);
	for (const key of Reflect.ownKeys(value)) {
		if (typeof key !== 'string') artifactInvalid(path);
		const descriptor = Object.getOwnPropertyDescriptor(value, key);
		if (
			descriptor === undefined ||
			!('value' in descriptor) ||
			descriptor.value === undefined
		) {
			artifactInvalid(`${path}.${key}`);
		}
		validateArtifactJson(
			descriptor.value,
			`${path}.${key}`,
			active,
			depth + 1
		);
	}
	active.delete(value);
}

