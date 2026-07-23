import type {
	DistributedCommandConsistency,
	DistributedCommandMetadata
} from '../protocol.js';
import { createCommandId } from '../causal.js';
import type { ReplicaValue } from './types.js';

const SHA256 = /^sha256:[0-9a-f]{64}$/;
const UUID_V7 =
	/^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;
const ULID = /^[0-7][0-9A-HJKMNP-TV-Z]{25}$/;
const GRAPHQL_NAME = /^[_A-Za-z][_0-9A-Za-z]*$/;
const ULID_ALPHABET = '0123456789ABCDEFGHJKMNPQRSTVWXYZ';
const MAX_TYPE_DEPTH = 64;

export type ReplicaCommandScalarCodec =
	| 'string'
	| 'string_unvalidated_timestamp'
	| 'base64'
	| 'boolean'
	| 'int32'
	| 'float64'
	| 'json_number_precision_limited'
	| 'json';

export type ReplicaCommandShape =
	| { readonly kind: 'none' }
	| { readonly kind: 'json'; readonly codec: 'json' }
	| {
			readonly kind: 'object';
			readonly definition: ReplicaCommandTypeDefinition;
	  };

export type ReplicaCommandTypeDefinition = {
	readonly name: string;
	readonly fields: readonly ReplicaCommandTypeField[];
};

export type ReplicaCommandTypeField = {
	readonly name: string;
	readonly typeName: string;
	readonly nullable: boolean;
	readonly list: boolean;
	readonly itemNullable: boolean;
	readonly codec?: ReplicaCommandScalarCodec;
	readonly nested?: ReplicaCommandTypeDefinition;
};

export type ReplicaCommandInputDefault = {
	readonly path: readonly string[];
	readonly generator: 'uuid_v7' | 'ulid';
};

export type ReplicaCommandInputDefaults = {
	readonly version: 1;
	readonly defaults: readonly ReplicaCommandInputDefault[];
};

export type ReplicaCommandEffectExpression =
	| { readonly kind: 'input'; readonly path: readonly string[] }
	| { readonly kind: 'trusted_preset'; readonly name: string }
	| { readonly kind: 'constant'; readonly value: ReplicaValue }
	| { readonly kind: 'null' };

export type ReplicaCommandEffectField = {
	readonly field: string;
	readonly value: ReplicaCommandEffectExpression;
};

export type ReplicaCommandEffectKey = {
	readonly fields: readonly ReplicaCommandEffectField[];
};

export type ReplicaCommandEffectRelationship = {
	readonly sourceModel: string;
	readonly field: string;
	readonly targetModel: string;
};

export type ReplicaCommandEffect =
	| {
			readonly kind: 'upsert' | 'patch';
			readonly model: string;
			readonly key: ReplicaCommandEffectKey;
			readonly fields: readonly ReplicaCommandEffectField[];
	  }
	| {
			readonly kind: 'delete';
			readonly model: string;
			readonly key: ReplicaCommandEffectKey;
	  }
	| {
			readonly kind: 'link' | 'unlink';
			readonly relationship: ReplicaCommandEffectRelationship;
			readonly source: ReplicaCommandEffectKey;
			readonly target: ReplicaCommandEffectKey;
	  }
	| {
			readonly kind: 'invalidate_model';
			readonly model: string;
	  }
	| {
			readonly kind: 'invalidate_relationship';
			readonly relationship: ReplicaCommandEffectRelationship;
			readonly source: ReplicaCommandEffectKey;
	  };

export type ReplicaCommandEffects = {
	readonly version: 1;
	readonly operations: readonly ReplicaCommandEffect[];
	readonly fallback: 'revalidate';
};

export type ReplicaCommandConfirmation = {
	readonly projector: string;
	readonly model: string;
	readonly key: ReplicaCommandEffectKey;
	readonly partition?: ReplicaCommandEffectExpression;
};

export type ReplicaCommandConfirmations =
	| {
			readonly version: 1;
			readonly kind: 'finite';
			readonly expected: readonly ReplicaCommandConfirmation[];
			readonly fallback: 'revalidate';
	  }
	| {
			readonly version: 1;
			readonly kind: 'unavailable';
			readonly expected: readonly [];
			readonly fallback: 'revalidate';
	  };

export type ReplicaCommandDirectProjection = {
	readonly topology: {
		readonly version: 1;
		readonly name: string;
		readonly digest: string;
	};
	readonly model: string;
	readonly identityFields: readonly string[];
	readonly partition?: ReplicaCommandEffectExpression;
	readonly changeEpoch: string;
};

/**
 * Conservative invalidation inventory emitted by the compiler.
 *
 * `dependencies` are the role-visible physical/model dependencies which task 9
 * may hand to the replica revalidator. `required` means the command cannot be
 * proven by its local effect/confirmation contract alone.
 */
export type ReplicaCommandRevalidationPlan = {
	readonly version: 1;
	readonly required: boolean;
	readonly dependencies: readonly string[];
	readonly models: readonly string[];
	readonly relationships: readonly ReplicaCommandEffectRelationship[];
};

/**
 * Framework-neutral executable command descriptor emitted by `dctl client`.
 *
 * The compiler has already validated the Rust-owned declaration. Runtime
 * validation remains fail-closed so a stale or hand-edited artifact cannot
 * create optimistic cache truth.
 */
export type ReplicaCommandArtifact<TInput = void, TOutput = unknown> = {
	readonly version: 1;
	readonly name: string;
	readonly mutationField: string;
	readonly document: string;
	readonly operationHash: string;
	readonly protocol: {
		readonly version: 2;
		readonly schemaHash: string;
		readonly protocolHash: string;
		readonly operation: string;
	};
	readonly input: ReplicaCommandShape;
	readonly output: ReplicaCommandShape;
	readonly inputDefaults?: ReplicaCommandInputDefaults;
	readonly consistency: DistributedCommandConsistency;
	readonly effects: ReplicaCommandEffects;
	readonly confirmations?: ReplicaCommandConfirmations;
	readonly directProjection?: ReplicaCommandDirectProjection;
	readonly revalidation: ReplicaCommandRevalidationPlan;
	/** Phantom slots preserve generated input/output types. */
	readonly __input?: TInput;
	readonly __output?: TOutput;
};

export type ReplicaPreparedEffectField = {
	readonly field: string;
	readonly value: ReplicaValue;
};

export type ReplicaPreparedEffectKey = {
	readonly fields: readonly ReplicaPreparedEffectField[];
};

export type ReplicaPreparedCommandEffect =
	| {
			readonly kind: 'upsert' | 'patch';
			readonly model: string;
			readonly key: ReplicaPreparedEffectKey;
			readonly fields: readonly ReplicaPreparedEffectField[];
	  }
	| {
			readonly kind: 'delete';
			readonly model: string;
			readonly key: ReplicaPreparedEffectKey;
	  }
	| {
			readonly kind: 'link' | 'unlink';
			readonly relationship: ReplicaCommandEffectRelationship;
			readonly source: ReplicaPreparedEffectKey;
			readonly target: ReplicaPreparedEffectKey;
	  }
	| {
			readonly kind: 'invalidate_model';
			readonly model: string;
	  }
	| {
			readonly kind: 'invalidate_relationship';
			readonly relationship: ReplicaCommandEffectRelationship;
			readonly source: ReplicaPreparedEffectKey;
	  };

export type ReplicaPreparedConfirmation = {
	readonly projector: string;
	readonly model: string;
	readonly key: ReplicaPreparedEffectKey;
	readonly partition?: ReplicaValue;
};

export type ReplicaPreparedConfirmations =
	| {
			readonly kind: 'finite';
			readonly expected: readonly ReplicaPreparedConfirmation[];
	  }
	| { readonly kind: 'unavailable' };

type DeepReadonly<T> = T extends (...args: never[]) => unknown
	? T
	: T extends readonly (infer TItem)[]
		? readonly DeepReadonly<TItem>[]
		: T extends object
			? { readonly [TKey in keyof T]: DeepReadonly<T[TKey]> }
			: T;

export type ReplicaCommandVariables<TInput> = [TInput] extends [void]
	? Readonly<{ commandId: string }>
	: Readonly<{ commandId: string; input: DeepReadonly<TInput> }>;

export type ReplicaPreparedCommand<TInput = void, TOutput = unknown> = {
	readonly name: string;
	readonly commandId: string;
	readonly consistency: DistributedCommandConsistency;
	readonly input: DeepReadonly<TInput>;
	readonly transport: {
		readonly mutationField: string;
		readonly document: string;
		readonly operationHash: string;
		readonly protocol: ReplicaCommandArtifact<TInput, TOutput>['protocol'];
		readonly variables: ReplicaCommandVariables<TInput>;
	};
	readonly optimistic: {
		readonly version: 1;
		readonly operations: readonly ReplicaPreparedCommandEffect[];
		readonly fallback: 'revalidate';
	};
	readonly confirmations?: ReplicaPreparedConfirmations;
	readonly directProjection?: {
		readonly topology: ReplicaCommandDirectProjection['topology'];
		readonly model: string;
		readonly identityFields: readonly string[];
		readonly partition?: ReplicaValue;
		readonly changeEpoch: string;
	};
	readonly revalidation: ReplicaCommandRevalidationPlan;
	/** Phantom output slot for generated wrappers. */
	readonly __output?: TOutput;
};

export type ReplicaCommandGenerators = {
	readonly uuidV7?: () => string;
	readonly ulid?: () => string;
};

export type PrepareReplicaCommandOptions = {
	readonly commandId?: string;
	readonly generators?: ReplicaCommandGenerators;
};

export type ReplicaReceiptVerification =
	| {
			readonly kind: 'matched';
			readonly revalidate: boolean;
	  }
	| {
			readonly kind: 'deferred';
			readonly revalidate: boolean;
	  }
	| {
			readonly kind: 'revalidate';
			readonly revalidate: true;
			readonly reason: 'confirmation_unavailable';
	  };

export type ReplicaCommandContractErrorCode =
	| 'REPLICA_COMMAND_ARTIFACT_INVALID'
	| 'REPLICA_COMMAND_INPUT_INVALID'
	| 'REPLICA_COMMAND_RECEIPT_MISMATCH';

/** Safe fail-closed error from a generated command descriptor seam. */
export class ReplicaCommandContractError extends Error {
	readonly code: ReplicaCommandContractErrorCode;
	readonly path: string;

	constructor(code: ReplicaCommandContractErrorCode, path: string) {
		super(commandErrorMessage(code, path));
		this.name = 'ReplicaCommandContractError';
		this.code = code;
		this.path = path;
	}
}

/**
 * Finalize one command before any overlay or network work begins.
 *
 * The returned object is a complete immutable retry unit. Generated defaults,
 * command identity, canonical variables, optimistic operations, projection
 * expressions, and revalidation inventory are never recomputed on reuse.
 */
export function prepareReplicaCommand<TInput, TOutput>(
	artifact: ReplicaCommandArtifact<TInput, TOutput>,
	input: TInput,
	options: PrepareReplicaCommandOptions = {}
): ReplicaPreparedCommand<TInput, TOutput> {
	validateArtifact(artifact);
	const defaults = artifact.inputDefaults?.defaults ?? [];
	const finalizedInput = materializeInput(
		artifact.input,
		input,
		defaults,
		options.generators
	) as DeepReadonly<TInput>;
	const commandId = validateUuidV7(
		options.commandId ?? createCommandId(),
		'options.commandId',
		'REPLICA_COMMAND_INPUT_INVALID'
	);
	const variables = Object.freeze({
		commandId,
		...(artifact.input.kind === 'none' ? {} : { input: finalizedInput })
	}) as ReplicaCommandVariables<TInput>;
	const operations = Object.freeze(
		artifact.effects.operations.map((effect, index) =>
			resolveEffect(effect, finalizedInput, `artifact.effects.operations[${index}]`)
		)
	);
	const confirmations = resolveConfirmations(
		artifact.confirmations,
		finalizedInput
	);
	const directProjection = resolveDirectProjection(
		artifact.directProjection,
		finalizedInput
	);
	const protocol = Object.freeze({ ...artifact.protocol });
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

function validateArtifact<TInput, TOutput>(
	artifact: ReplicaCommandArtifact<TInput, TOutput>
): void {
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
	if (artifact.protocol.operation !== artifact.operationHash) {
		artifactInvalid('artifact.protocol.operation');
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
}

function validateShape(
	shape: ReplicaCommandShape,
	path: string,
	depth: number,
	active: Set<ReplicaCommandTypeDefinition>
): void {
	if (depth > MAX_TYPE_DEPTH) artifactInvalid(path);
	switch (shape.kind) {
		case 'none':
			return;
		case 'json':
			if (shape.codec !== 'json') artifactInvalid(`${path}.codec`);
			return;
		case 'object':
			validateDefinition(shape.definition, `${path}.definition`, depth, active);
			return;
		default:
			artifactInvalid(`${path}.kind`);
	}
}

function validateDefinition(
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

function validateDefaults(
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

function validateConfirmations<TInput, TOutput>(
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

function validateDirectProjection<TInput, TOutput>(
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

function validateRevalidation<TInput, TOutput>(
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
			artifact.confirmations?.kind === 'unavailable') &&
		!plan.required
	) {
		artifactInvalid('artifact.revalidation.required');
	}
}

function validateEffectArtifact(
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

function validateKeyArtifact(
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

function validateFieldsArtifact(
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

function validateExpressionArtifact(
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
			// Task 10 owns installing a cache-scope-bound preset inventory.
			artifactInvalid(path);
		case undefined:
		default:
			artifactInvalid(`${path}.kind`);
	}
}

function validateRelationshipArtifact(
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

function validateArtifactJson(
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

function materializeInput(
	shape: ReplicaCommandShape,
	input: unknown,
	defaults: readonly ReplicaCommandInputDefault[],
	generators: ReplicaCommandGenerators | undefined
): unknown {
	switch (shape.kind) {
		case 'none':
			if (input !== undefined) inputInvalid('input');
			return undefined;
		case 'json':
			if (input === undefined) inputInvalid('input');
			return cloneJson(input, 'input');
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

function cloneDefinitionValue(
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

function cloneFieldValue(
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

function cloneNonNullFieldValue(
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

function cloneScalar(
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

function cloneJson(
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

function resolveEffect(
	effect: ReplicaCommandEffect,
	input: unknown,
	path: string
): ReplicaPreparedCommandEffect {
	switch (effect.kind) {
		case 'upsert':
		case 'patch':
			return Object.freeze({
				kind: effect.kind,
				model: requiredString(effect.model, `${path}.model`),
				key: resolveKey(effect.key, input, `${path}.key`),
				fields: resolveFields(effect.fields, input, `${path}.fields`)
			});
		case 'delete':
			return Object.freeze({
				kind: effect.kind,
				model: requiredString(effect.model, `${path}.model`),
				key: resolveKey(effect.key, input, `${path}.key`)
			});
		case 'link':
		case 'unlink':
			return Object.freeze({
				kind: effect.kind,
				relationship: cloneRelationship(
					effect.relationship,
					`${path}.relationship`
				),
				source: resolveKey(effect.source, input, `${path}.source`),
				target: resolveKey(effect.target, input, `${path}.target`)
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
				source: resolveKey(effect.source, input, `${path}.source`)
			});
		default:
			artifactInvalid(`${path}.kind`);
	}
}

function resolveKey(
	key: ReplicaCommandEffectKey,
	input: unknown,
	path: string
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
		fields: resolveFields(key.fields, input, `${path}.fields`)
	});
}

function resolveFields(
	fields: readonly ReplicaCommandEffectField[],
	input: unknown,
	path: string
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
				value: resolveExpression(field.value, input, `${fieldPath}.value`)
			});
		})
	);
}

function resolveExpression(
	expression: ReplicaCommandEffectExpression,
	input: unknown,
	path: string
): ReplicaValue {
	switch (expression.kind) {
		case 'input':
			return resolveInputPath(input, expression.path, path);
		case 'constant':
			return cloneJson(expression.value, `${path}.value`);
		case 'null':
			return null;
		case 'trusted_preset':
			artifactInvalid(path);
		default:
			artifactInvalid(`${path}.kind`);
	}
}

function resolveInputPath(
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

function resolveConfirmations(
	confirmations: ReplicaCommandConfirmations | undefined,
	input: unknown
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
								`${path}.partition`
							);
				return Object.freeze({
					projector: requiredString(
						confirmation.projector,
						`${path}.projector`
					),
					model: requiredString(confirmation.model, `${path}.model`),
					key: resolveKey(confirmation.key, input, `${path}.key`),
					...(partition === undefined ? {} : { partition })
				});
			})
		)
	});
}

function resolveDirectProjection(
	direct: ReplicaCommandDirectProjection | undefined,
	input: unknown
): ReplicaPreparedCommand<unknown>['directProjection'] | undefined {
	if (direct === undefined) return undefined;
	const partition =
		direct.partition === undefined
			? undefined
			: resolveExpression(
					direct.partition,
					input,
					'artifact.directProjection.partition'
				);
	return Object.freeze({
		topology: Object.freeze({ ...direct.topology }),
		model: direct.model,
		identityFields: Object.freeze([...direct.identityFields]),
		...(partition === undefined ? {} : { partition }),
		changeEpoch: direct.changeEpoch
	});
}

function cloneRevalidation(
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

function cloneRelationship(
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

function fieldAtPath(
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

function generateDefault(
	entry: ReplicaCommandInputDefault,
	generators: ReplicaCommandGenerators | undefined,
	path: string
): string {
	switch (entry.generator) {
		case 'uuid_v7':
			return validateUuidV7(
				(generators?.uuidV7 ?? createCommandId)(),
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

function createUlid(): string {
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

function validateUuidV7(
	value: unknown,
	path: string,
	code: ReplicaCommandContractErrorCode
): string {
	if (typeof value !== 'string' || !UUID_V7.test(value)) {
		throw new ReplicaCommandContractError(code, path);
	}
	return value.toLowerCase();
}

function validateUlid(value: unknown, path: string): string {
	if (typeof value !== 'string' || !ULID.test(value.toUpperCase())) {
		inputInvalid(path);
	}
	return value.toUpperCase();
}

function sameProjectionMultiset(
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

function samePath(left: readonly string[], right: readonly string[]): boolean {
	return (
		left.length === right.length &&
		left.every((segment, index) => segment === right[index])
	);
}

function isSupportedCodec(value: string): value is ReplicaCommandScalarCodec {
	return (
		value === 'string' ||
		value === 'string_unvalidated_timestamp' ||
		value === 'base64' ||
		value === 'boolean' ||
		value === 'int32' ||
		value === 'float64' ||
		value === 'json_number_precision_limited' ||
		value === 'json'
	);
}

function isPlainRecord(value: unknown): value is Record<string, unknown> {
	if (value === null || typeof value !== 'object' || Array.isArray(value)) {
		return false;
	}
	const prototype = Object.getPrototypeOf(value);
	return prototype === Object.prototype || prototype === null;
}

function defineEnumerable<T>(
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

function nonempty(value: unknown, path: string): asserts value is string {
	if (typeof value !== 'string' || value.trim() === '') artifactInvalid(path);
}

function requiredString(value: unknown, path: string): string {
	nonempty(value, path);
	return value;
}

function hash(value: unknown, path: string): void {
	if (typeof value !== 'string' || !SHA256.test(value)) artifactInvalid(path);
}

function artifactInvalid(path: string): never {
	throw new ReplicaCommandContractError(
		'REPLICA_COMMAND_ARTIFACT_INVALID',
		path
	);
}

function inputInvalid(path: string): never {
	throw new ReplicaCommandContractError('REPLICA_COMMAND_INPUT_INVALID', path);
}

function receiptMismatch(path: string): never {
	throw new ReplicaCommandContractError(
		'REPLICA_COMMAND_RECEIPT_MISMATCH',
		path
	);
}

function commandErrorMessage(
	code: ReplicaCommandContractErrorCode,
	path: string
): string {
	switch (code) {
		case 'REPLICA_COMMAND_ARTIFACT_INVALID':
			return `Invalid generated replica command artifact at ${path}`;
		case 'REPLICA_COMMAND_INPUT_INVALID':
			return `Invalid replica command input at ${path}`;
		case 'REPLICA_COMMAND_RECEIPT_MISMATCH':
			return `Replica command receipt does not match ${path}`;
	}
}
