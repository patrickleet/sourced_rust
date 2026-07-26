import {
	isDistributedTrustedPresetCodec,
	parseDistributedTrustedPresetInventory,
	type DistributedTrustedPreset,
	type DistributedCommandMetadata
} from '../../protocol.js';
import type { ReplicaClientSurface, ReplicaValue } from '../types.js';
import { createReplicaCommandId } from '../command-id.js';
import { isPlainRecord } from '../../lib/is-plain-record.js';
import {
	GRAPHQL_NAME,
	MAX_TYPE_DEPTH,
	SHA256,
	ULID,
	ULID_ALPHABET,
	UUID_V7
} from './constants.js';
import type {
	DeepReadonly,
	PrepareReplicaCommandOptions,
	ReplicaCommandArtifact,
	ReplicaCommandConfirmations,
	ReplicaCommandContractErrorCode,
	ReplicaCommandDirectProjection,
	ReplicaCommandEffect,
	ReplicaCommandEffectExpression,
	ReplicaCommandEffectField,
	ReplicaCommandEffectKey,
	ReplicaCommandEffectRelationship,
	ReplicaCommandGenerators,
	ReplicaCommandInputDefault,
	ReplicaCommandInputDefaults,
	ReplicaCommandRevalidationPlan,
	ReplicaCommandScalarCodec,
	ReplicaCommandShape,
	ReplicaCommandTypeDefinition,
	ReplicaCommandTypeField,
	ReplicaCommandVariables,
	ReplicaMatchedTrustedPresetInventory,
	ReplicaPreparedCommand,
	ReplicaPreparedCommandEffect,
	ReplicaPreparedConfirmations,
	ReplicaPreparedEffectField,
	ReplicaPreparedEffectKey,
	ReplicaReceiptVerification,
	ReplicaTrustedPresetDescriptor,
	TrustedPresetReference
} from './types.js';
import { ReplicaCommandContractError } from './errors.js';

export function prepareReplicaCommand<TInput, TOutput>(
	artifact: ReplicaCommandArtifact<TInput, TOutput>,
	input: TInput,
	options: PrepareReplicaCommandOptions = {}
): ReplicaPreparedCommand<TInput, TOutput> {
	return prepareReplicaCommandInternal(artifact, input, options);
}

/**
 * Finalize a command with values obtained from the replica's current
 * authoritative scope generation.
 *
 * The incoming inventory is scope-wide, so values owned by other commands are
 * ignored. Every descriptor consumed by this artifact must nevertheless have
 * one exact name/codec match before defaults, optimism, or transport state is
 * created.
 *
 * This seam is package-internal and deliberately omitted from the public
 * `@hops-ops/distributed/replica` entry point.
 *
 * @internal
 */
export function prepareReplicaCommandWithTrustedPresets<TInput, TOutput>(
	artifact: ReplicaCommandArtifact<TInput, TOutput>,
	input: TInput,
	authoritativePresets: readonly DistributedTrustedPreset[],
	options: PrepareReplicaCommandOptions = {}
): ReplicaPreparedCommand<TInput, TOutput> {
	return prepareReplicaCommandInternal(
		artifact,
		input,
		options,
		authoritativePresets
	);
}

export function prepareReplicaCommandInternal<TInput, TOutput>(
	artifact: ReplicaCommandArtifact<TInput, TOutput>,
	input: TInput,
	options: PrepareReplicaCommandOptions,
	authoritativePresets?: readonly DistributedTrustedPreset[]
): ReplicaPreparedCommand<TInput, TOutput> {
	const descriptors = validateArtifact(
		artifact,
		authoritativePresets !== undefined
	);
	const trustedPresets =
		authoritativePresets === undefined
			? undefined
			: selectReplicaTrustedPresetInventory(descriptors, authoritativePresets);
	const defaults = artifact.inputDefaults?.defaults ?? [];
	const finalizedInput = materializeInput(
		artifact.input,
		input,
		defaults,
		options.generators
	) as DeepReadonly<TInput>;
	const commandId = validateUuidV7(
		options.commandId ?? createReplicaCommandId(),
		'options.commandId',
		'REPLICA_COMMAND_INPUT_INVALID'
	);
	const variables = Object.freeze({
		commandId,
		...(artifact.input.kind === 'none' ? {} : { input: finalizedInput })
	}) as ReplicaCommandVariables<TInput>;
	const operations = Object.freeze(
		artifact.effects.operations.map((effect, index) =>
			resolveEffect(
				effect,
				finalizedInput,
				`artifact.effects.operations[${index}]`,
				trustedPresets
			)
		)
	);
	const confirmations = resolveConfirmations(
		artifact.confirmations,
		finalizedInput,
		trustedPresets
	);
	const directProjection = resolveDirectProjection(
		artifact.directProjection,
		finalizedInput,
		trustedPresets
	);
	const protocol = Object.freeze({
		...artifact.protocol,
		surface: cloneClientSurface(artifact.protocol.surface),
		trustedPresets: cloneTrustedPresetDescriptors(
			artifact.protocol.trustedPresets
		)
	});
	const revalidation = cloneRevalidation(artifact.revalidation);

	return Object.freeze({
		name: artifact.name,
		commandId,
		consistency: artifact.consistency,
		input: finalizedInput,
		transport: Object.freeze({
			mutationField: artifact.mutationField,
			document: artifact.document,
			operationHash: artifact.operationHash,
			protocol,
			variables
		}),
		optimistic: Object.freeze({
			version: 1 as const,
			operations,
			fallback: 'revalidate' as const
		}),
		...(confirmations === undefined ? {} : { confirmations }),
		...(directProjection === undefined ? {} : { directProjection }),
		revalidation
	}) as ReplicaPreparedCommand<TInput, TOutput>;
}

/**
 * Match two command-local inventories as exact name/codec sets.
 *
 * Both sides are reparsed instead of trusting TypeScript annotations. Missing,
 * extra, duplicate, unsupported, or codec-mismatched entries fail closed.
 *
 * @internal
 */
export function matchReplicaTrustedPresetInventory(
	expected: readonly ReplicaTrustedPresetDescriptor[],
	authoritative: readonly DistributedTrustedPreset[]
): ReplicaMatchedTrustedPresetInventory {
	const descriptors = parseReplicaTrustedPresetDescriptors(expected);
	let values: readonly DistributedTrustedPreset[];
	try {
		values = parseDistributedTrustedPresetInventory(
			authoritative,
			'authoritativePresets'
		);
	} catch {
		trustedPresetMismatch('authoritativePresets');
	}
	if (descriptors.length !== values.length) {
		trustedPresetMismatch('authoritativePresets');
	}
	const byName = new Map(values.map((preset) => [preset.name, preset] as const));
	for (let index = 0; index < descriptors.length; index += 1) {
		const descriptor = descriptors[index]!;
		const value = byName.get(descriptor.name);
		if (value === undefined || value.codec !== descriptor.codec) {
			trustedPresetMismatch(`authoritativePresets.${descriptor.name}`);
		}
	}
	const resolve = (name: string): ReplicaValue => {
		const value = byName.get(name);
		if (value === undefined) {
			trustedPresetMismatch(`authoritativePresets.${name}`);
		}
		return value.value as ReplicaValue;
	};
	return Object.freeze({
		descriptors,
		values,
		resolve
	});
}

export function selectReplicaTrustedPresetInventory(
	expected: readonly ReplicaTrustedPresetDescriptor[],
	authoritative: readonly DistributedTrustedPreset[]
): ReplicaMatchedTrustedPresetInventory {
	let values: readonly DistributedTrustedPreset[];
	try {
		values = parseDistributedTrustedPresetInventory(
			authoritative,
			'authoritativePresets'
		);
	} catch {
		trustedPresetMismatch('authoritativePresets');
	}
	const names = new Set(expected.map(({ name }) => name));
	return matchReplicaTrustedPresetInventory(
		expected,
		values.filter(({ name }) => names.has(name))
	);
}

/**
 * Verify one parsed server receipt against the immutable prepared contract.
 *
 * This does not drive command status or overlay retirement. It only decides
 * whether task 9 may safely consume the receipt as matching evidence.
 */
export function verifyReplicaCommandReceipt<TInput, TOutput>(
	prepared: ReplicaPreparedCommand<TInput, TOutput>,
	receipt: DistributedCommandMetadata
): ReplicaReceiptVerification {
	if (receipt.commandId !== prepared.commandId) {
		receiptMismatch('receipt.commandId');
	}
	if (receipt.consistency !== prepared.consistency) {
		receiptMismatch('receipt.consistency');
	}

	const contract = prepared.confirmations;
	if (contract?.kind === 'unavailable') {
		return Object.freeze({
			kind: 'revalidate',
			revalidate: true,
			reason: 'confirmation_unavailable'
		});
	}

	const expected =
		contract?.kind === 'finite'
			? contract.expected.map(({ projector, model }) => ({
					projection: projector,
					model
				}))
			: [];
	if (receipt.state === 'in_progress' && receipt.expects.length === 0) {
		return Object.freeze({
			kind: 'deferred',
			revalidate: prepared.revalidation.required
		});
	}
	if (!sameProjectionMultiset(expected, receipt.expects)) {
		receiptMismatch('receipt.expects');
	}
	return Object.freeze({
		kind: receipt.state === 'in_progress' ? 'deferred' : 'matched',
		revalidate: prepared.revalidation.required
	});
}

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

export function cloneClientSurface(surface: ReplicaClientSurface): ReplicaClientSurface {
	return surface.kind === 'role'
		? Object.freeze({ kind: 'role' as const, name: surface.name })
		: Object.freeze({
				kind: 'application' as const,
				name: surface.name,
				roles: Object.freeze([...surface.roles])
			});
}

export function parseReplicaTrustedPresetDescriptors(
	value: unknown,
	path = 'artifact.trustedPresets'
): readonly ReplicaTrustedPresetDescriptor[] {
	if (!Array.isArray(value)) artifactInvalid(path);
	const names = new Set<string>();
	return Object.freeze(
		value.map((candidate, index) => {
			const itemPath = `${path}[${index}]`;
			if (!isPlainRecord(candidate)) artifactInvalid(itemPath);
			const name = trustedPresetName(candidate.name, `${itemPath}.name`);
			if (names.has(name)) artifactInvalid(`${itemPath}.name`);
			names.add(name);
			if (!isDistributedTrustedPresetCodec(candidate.codec)) {
				artifactInvalid(`${itemPath}.codec`);
			}
			return Object.freeze({
				name,
				codec: candidate.codec
			});
		})
	);
}

export function cloneTrustedPresetDescriptors(
	value: readonly ReplicaTrustedPresetDescriptor[]
): readonly ReplicaTrustedPresetDescriptor[] {
	return Object.freeze(
		value.map(({ name, codec }) => Object.freeze({ name, codec }))
	);
}

export function trustedPresetName(value: unknown, path: string): string {
	if (
		typeof value !== 'string' ||
		value.length === 0 ||
		value.length > 128 ||
		value.trim() !== value ||
		/[\u0000-\u001f\u007f-\u009f]/.test(value)
	) {
		artifactInvalid(path);
	}
	return value;
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

export function cloneDefinitionValue(
	definition: ReplicaCommandTypeDefinition,
	value: unknown,
	path: readonly string[],
	defaults: readonly ReplicaCommandInputDefault[],
	generators: ReplicaCommandGenerators | undefined,
	label: string
): Readonly<Record<string, unknown>> {
	if (!isPlainRecord(value)) inputInvalid(label);
	const ownKeys = Reflect.ownKeys(value);
	if (ownKeys.some((key) => typeof key !== 'string')) inputInvalid(label);
	const known = new Set(definition.fields.map((field) => field.name));
	for (const key of ownKeys as string[]) {
		if (!known.has(key)) inputInvalid(`${label}.${key}`);
		const descriptor = Object.getOwnPropertyDescriptor(value, key);
		if (descriptor === undefined || !('value' in descriptor)) {
			inputInvalid(`${label}.${key}`);
		}
	}

	const result: Record<string, unknown> = {};
	for (const field of definition.fields) {
		const fieldPath = [...path, field.name];
		const fieldLabel = `${label}.${field.name}`;
		const present =
			Object.prototype.hasOwnProperty.call(value, field.name) &&
			value[field.name] !== undefined;
		let fieldValue: unknown;
		if (present) {
			fieldValue = value[field.name];
		} else {
			const declared = defaults.find((entry) =>
				samePath(entry.path, fieldPath)
			);
			if (declared !== undefined) {
				fieldValue = generateDefault(declared, generators, fieldLabel);
			} else if (field.nullable) {
				continue;
			} else {
				inputInvalid(fieldLabel);
			}
		}
		defineEnumerable(
			result,
			field.name,
			cloneFieldValue(
				field,
				fieldValue,
				fieldPath,
				defaults,
				generators,
				fieldLabel
			)
		);
	}
	return Object.freeze(result);
}

export function cloneFieldValue(
	field: ReplicaCommandTypeField,
	value: unknown,
	path: readonly string[],
	defaults: readonly ReplicaCommandInputDefault[],
	generators: ReplicaCommandGenerators | undefined,
	label: string
): unknown {
	if (value === null) {
		if (!field.nullable) inputInvalid(label);
		return null;
	}
	if (field.list) {
		if (!Array.isArray(value)) inputInvalid(label);
		return Object.freeze(
			value.map((item, index) => {
				if (item === null) {
					if (!field.itemNullable) inputInvalid(`${label}[${index}]`);
					return null;
				}
				return cloneNonNullFieldValue(
					field,
					item,
					path,
					defaults,
					generators,
					`${label}[${index}]`
				);
			})
		);
	}
	return cloneNonNullFieldValue(
		field,
		value,
		path,
		defaults,
		generators,
		label
	);
}

export function cloneNonNullFieldValue(
	field: ReplicaCommandTypeField,
	value: unknown,
	path: readonly string[],
	defaults: readonly ReplicaCommandInputDefault[],
	generators: ReplicaCommandGenerators | undefined,
	label: string
): unknown {
	if (field.nested !== undefined) {
		return cloneDefinitionValue(
			field.nested,
			value,
			path,
			defaults,
			generators,
			label
		);
	}
	return cloneScalar(field.codec, value, label);
}

export function cloneScalar(
	codec: ReplicaCommandScalarCodec | undefined,
	value: unknown,
	path: string
): ReplicaValue {
	switch (codec) {
		case 'string':
		case 'string_unvalidated_timestamp':
		case 'base64':
			if (typeof value !== 'string') inputInvalid(path);
			return value;
		case 'boolean':
			if (typeof value !== 'boolean') inputInvalid(path);
			return value;
		case 'int32':
			if (
				typeof value !== 'number' ||
				!Number.isInteger(value) ||
				value < -2_147_483_648 ||
				value > 2_147_483_647
			) {
				inputInvalid(path);
			}
			return Object.is(value, -0) ? 0 : value;
		case 'json_number_precision_limited':
			if (typeof value !== 'number' || !Number.isInteger(value)) {
				inputInvalid(path);
			}
			return Object.is(value, -0) ? 0 : value;
		case 'float64':
			if (typeof value !== 'number' || !Number.isFinite(value)) {
				inputInvalid(path);
			}
			return Object.is(value, -0) ? 0 : value;
		case 'json':
			return cloneJson(value, path);
		default:
			artifactInvalid(`${path}.codec`);
	}
}

export function cloneJson(
	value: unknown,
	path: string,
	active: Set<object> = new Set(),
	depth = 0
): ReplicaValue {
	if (depth > MAX_TYPE_DEPTH) inputInvalid(path);
	if (
		value === null ||
		typeof value === 'string' ||
		typeof value === 'boolean'
	) {
		return value;
	}
	if (typeof value === 'number') {
		if (!Number.isFinite(value)) inputInvalid(path);
		return Object.is(value, -0) ? 0 : value;
	}
	if (Array.isArray(value)) {
		if (active.has(value)) inputInvalid(path);
		active.add(value);
		const result = Object.freeze(
			value.map((item, index) =>
				cloneJson(item, `${path}[${index}]`, active, depth + 1)
			)
		);
		active.delete(value);
		return result;
	}
	if (!isPlainRecord(value)) inputInvalid(path);
	if (active.has(value)) inputInvalid(path);
	active.add(value);
	const keys = Reflect.ownKeys(value);
	if (keys.some((key) => typeof key !== 'string')) inputInvalid(path);
	const result: Record<string, ReplicaValue> = {};
	for (const key of (keys as string[]).sort()) {
		const descriptor = Object.getOwnPropertyDescriptor(value, key);
		if (
			descriptor === undefined ||
			!('value' in descriptor) ||
			descriptor.value === undefined
		) {
			inputInvalid(`${path}.${key}`);
		}
		defineEnumerable(
			result,
			key,
			cloneJson(descriptor.value, `${path}.${key}`, active, depth + 1)
		);
	}
	active.delete(value);
	return Object.freeze(result);
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

export function cloneRevalidation(
	plan: ReplicaCommandRevalidationPlan
): ReplicaCommandRevalidationPlan {
	return Object.freeze({
		version: 1 as const,
		required: plan.required,
		dependencies: Object.freeze([...plan.dependencies]),
		models: Object.freeze([...plan.models]),
		relationships: Object.freeze(
			plan.relationships.map((relationship, index) =>
				cloneRelationship(
					relationship,
					`artifact.revalidation.relationships[${index}]`
				)
			)
		)
	});
}

export function cloneRelationship(
	relationship: ReplicaCommandEffectRelationship,
	path: string
): ReplicaCommandEffectRelationship {
	return Object.freeze({
		sourceModel: requiredString(
			relationship.sourceModel,
			`${path}.sourceModel`
		),
		field: requiredString(relationship.field, `${path}.field`),
		targetModel: requiredString(
			relationship.targetModel,
			`${path}.targetModel`
		)
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

export function generateDefault(
	entry: ReplicaCommandInputDefault,
	generators: ReplicaCommandGenerators | undefined,
	path: string
): string {
	switch (entry.generator) {
			case 'uuid_v7':
				return validateUuidV7(
					(generators?.uuidV7 ?? createReplicaCommandId)(),
					path,
					'REPLICA_COMMAND_INPUT_INVALID'
				);
		case 'ulid':
			return validateUlid(
				(generators?.ulid ?? createUlid)(),
				path
			);
		default:
			artifactInvalid(`${path}.generator`);
	}
}

export function createUlid(): string {
	const crypto = globalThis.crypto;
	if (!crypto || typeof crypto.getRandomValues !== 'function') {
		throw new ReplicaCommandContractError(
			'REPLICA_COMMAND_INPUT_INVALID',
			'inputDefaults.ulid'
		);
	}
	let timestamp = Date.now();
	let time = '';
	for (let index = 0; index < 10; index += 1) {
		time = ULID_ALPHABET[timestamp % 32]! + time;
		timestamp = Math.floor(timestamp / 32);
	}
	const bytes = crypto.getRandomValues(new Uint8Array(10));
	let random = 0n;
	for (const byte of bytes) random = (random << 8n) | BigInt(byte);
	let suffix = '';
	for (let index = 0; index < 16; index += 1) {
		suffix = ULID_ALPHABET[Number(random & 31n)]! + suffix;
		random >>= 5n;
	}
	return `${time}${suffix}`;
}

export function validateUuidV7(
	value: unknown,
	path: string,
	code: ReplicaCommandContractErrorCode
): string {
	if (typeof value !== 'string' || !UUID_V7.test(value)) {
		throw new ReplicaCommandContractError(code, path);
	}
	return value.toLowerCase();
}

export function validateUlid(value: unknown, path: string): string {
	if (typeof value !== 'string' || !ULID.test(value.toUpperCase())) {
		inputInvalid(path);
	}
	return value.toUpperCase();
}

export function sameProjectionMultiset(
	expected: readonly { projection: string; model: string }[],
	actual: readonly { projection: string; model: string }[]
): boolean {
	if (expected.length !== actual.length) return false;
	const counts = new Map<string, number>();
	for (const item of expected) {
		const key = JSON.stringify([item.projection, item.model]);
		counts.set(key, (counts.get(key) ?? 0) + 1);
	}
	for (const item of actual) {
		const key = JSON.stringify([item.projection, item.model]);
		const count = counts.get(key);
		if (count === undefined) return false;
		if (count === 1) counts.delete(key);
		else counts.set(key, count - 1);
	}
	return counts.size === 0;
}

export function samePath(left: readonly string[], right: readonly string[]): boolean {
	return (
		left.length === right.length &&
		left.every((segment, index) => segment === right[index])
	);
}

export function isSupportedCodec(value: string): value is ReplicaCommandScalarCodec {
	return isDistributedTrustedPresetCodec(value);
}

export function defineEnumerable<T>(
	target: Record<string, T>,
	key: string,
	value: T
): void {
	Object.defineProperty(target, key, {
		value,
		enumerable: true,
		configurable: true,
		writable: true
	});
}

export function nonempty(value: unknown, path: string): asserts value is string {
	if (typeof value !== 'string' || value.trim() === '') artifactInvalid(path);
}

export function requiredString(value: unknown, path: string): string {
	nonempty(value, path);
	return value;
}

export function hash(value: unknown, path: string): void {
	if (typeof value !== 'string' || !SHA256.test(value)) artifactInvalid(path);
}

export function artifactInvalid(path: string): never {
	throw new ReplicaCommandContractError(
		'REPLICA_COMMAND_ARTIFACT_INVALID',
		path
	);
}

export function inputInvalid(path: string): never {
	throw new ReplicaCommandContractError('REPLICA_COMMAND_INPUT_INVALID', path);
}

export function receiptMismatch(path: string): never {
	throw new ReplicaCommandContractError(
		'REPLICA_COMMAND_RECEIPT_MISMATCH',
		path
	);
}

export function trustedPresetMismatch(path: string): never {
	throw new ReplicaCommandContractError(
		'REPLICA_COMMAND_TRUSTED_PRESET_MISMATCH',
		path
	);
}

