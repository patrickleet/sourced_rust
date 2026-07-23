import { cacheIndexKey } from '../internal/cache-engine.js';
import type { GraphqlVariables } from '../types.js';
import type {
	ReplicaArgumentValue,
	ReplicaArgumentsArtifact,
	ReplicaCoverageArtifact,
	ReplicaIdentity,
	ReplicaIndexCoverage,
	ReplicaModelArtifact,
	ReplicaOperationArtifact,
	ReplicaVariableCodecArtifact,
	ReplicaVariableCodecLimits,
	ReplicaVariableFilterInputDefinition,
	ReplicaVariableFilterInputField,
	ReplicaVariableInputDefinition,
	ReplicaVariableInputRef,
	ReplicaVariableOrderInputDefinition,
	ReplicaValue
} from './types.js';

const GRAPHQL_NAME = /^[_A-Za-z][_0-9A-Za-z]*$/;
const MAX_VARIABLE_CODEC_DEPTH = 64;
const FILTER_OPERATORS = new Set([
	'_eq',
	'_neq',
	'_gt',
	'_gte',
	'_lt',
	'_lte',
	'_in',
	'_nin',
	'_is_null',
	'_like',
	'_ilike',
	'_contains',
	'_contained_in',
	'_has_key'
]);

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
	variables: GraphqlVariables,
	coverage?: ReplicaCoverageArtifact
): Readonly<Record<string, ReplicaValue>> {
	const entries: Array<readonly [string, ReplicaValue]> = [];
	for (const [argument, source] of Object.entries(artifact ?? {})) {
		assertName(argument, 'GraphQL argument');
		const value = resolveReplicaArgumentValue(source, variables);
		if (value !== undefined) entries.push([argument, value]);
	}
	const resolved = freezeRecord(entries);
	return coverage?.kind === 'offset'
		? effectiveOffsetArguments(resolved, coverage)
		: resolved;
}

export function resolveReplicaArgumentValue(
	source: ReplicaArgumentValue,
	variables: GraphqlVariables,
	position: 'argument' | 'object' | 'list' = 'argument'
): ReplicaValue | undefined {
	if (source.kind === 'literal') return cloneJsonValue(source.value);
	if (source.kind === 'variable') {
		assertName(source.name, 'GraphQL variable');
		if (
			!Object.prototype.hasOwnProperty.call(variables, source.name) ||
			variables[source.name] === undefined
		) {
			return position === 'list' ? null : undefined;
		}
		return cloneJsonValue(variables[source.name]);
	}
	if (source.kind === 'list') {
		if (!Array.isArray(source.items)) {
			throw new TypeError('GraphQL list argument source must contain an items array');
		}
		return Object.freeze(
			source.items.map(
				(item) => resolveReplicaArgumentValue(item, variables, 'list') ?? null
			)
		);
	}
	if (source.kind === 'object') {
		if (
			source.fields === null ||
			typeof source.fields !== 'object' ||
			Array.isArray(source.fields)
		) {
			throw new TypeError('GraphQL object argument source must contain a fields object');
		}
		const entries: Array<readonly [string, ReplicaValue]> = [];
		for (const [field, value] of Object.entries(source.fields)) {
			assertName(field, 'GraphQL input field');
			const resolved = resolveReplicaArgumentValue(value, variables, 'object');
			if (resolved !== undefined) entries.push([field, resolved]);
		}
		return freezeRecord(entries);
	}
	throw new TypeError('unsupported argument source');
}

type VariableCodecRegistry = {
	readonly limits: ReplicaVariableCodecLimits;
	readonly variables: ReadonlyMap<string, ReplicaVariableInputRef>;
	readonly inputs: ReadonlyMap<string, ReplicaVariableInputDefinition>;
};

/**
 * Apply the exact compiler-owned GraphQL input coercion contract before an
 * operation can acquire cache identity or reach a transport.
 */
export function canonicalizeOperationVariables<
	TData,
	TVariables extends GraphqlVariables
>(
	artifact: ReplicaOperationArtifact<TData, TVariables>,
	variables: TVariables
): TVariables {
	const codec = artifact.variableCodec;
	if (codec === undefined) {
		if (artifact.protocol !== undefined) {
			throw new TypeError('protocol-v2 replica artifact requires variableCodec');
		}
		return cloneJsonObject(variables) as TVariables;
	}

	const registry = validateVariableCodec(codec);
	const supplied = new Map(
		inputRecordEntries(variables, 'variables').map(([name, value]) => {
			if (!registry.variables.has(name)) {
				variableValueInvalid(`variables.${name}`, 'unknown operation variable');
			}
			return [name, value] as const;
		})
	);
	const canonical: Array<readonly [string, ReplicaValue]> = [];
	for (const [name, input] of [...registry.variables].sort(([left], [right]) =>
		compareCodeUnits(left, right)
	)) {
		const present = supplied.has(name) && supplied.get(name) !== undefined;
		if (!present) {
			if (!input.nullable) {
				variableValueInvalid(`variables.${name}`, 'required variable is missing');
			}
			continue;
		}
		canonical.push([
			name,
			canonicalizeInputRef(
				input,
				supplied.get(name),
				registry,
				`variables.${name}`,
				new Set(),
				0
			)
		]);
	}
	return freezeRecord(canonical) as TVariables;
}

function validateVariableCodec(codec: ReplicaVariableCodecArtifact): VariableCodecRegistry {
	const root = artifactRecord(
		codec,
		'artifact.variableCodec',
		['version', 'limits', 'variables', 'inputs']
	);
	if (root.version !== 2) variableCodecInvalid('artifact.variableCodec.version');
	const rawLimits = artifactRecord(
		root.limits,
		'artifact.variableCodec.limits',
		['maxDepth', 'maxBoolWidth', 'maxInList']
	);
	const limits: ReplicaVariableCodecLimits = {
		maxDepth: validateCodecLimit(
			rawLimits.maxDepth,
			'artifact.variableCodec.limits.maxDepth'
		),
		maxBoolWidth: validateCodecLimit(
			rawLimits.maxBoolWidth,
			'artifact.variableCodec.limits.maxBoolWidth'
		),
		maxInList: validateCodecLimit(
			rawLimits.maxInList,
			'artifact.variableCodec.limits.maxInList'
		)
	};

	const inputEntries = artifactRecordEntries(root.inputs, 'artifact.variableCodec.inputs');
	const inputs = new Map<string, ReplicaVariableInputDefinition>();
	for (const [name, definition] of inputEntries) {
		assertGraphqlName(name, `artifact.variableCodec.inputs.${name}`);
		inputs.set(name, definition as ReplicaVariableInputDefinition);
	}

	const variableEntries = artifactRecordEntries(
		root.variables,
		'artifact.variableCodec.variables'
	);
	const variables = new Map<string, ReplicaVariableInputRef>();
	for (const [name, input] of variableEntries) {
		assertGraphqlName(name, `artifact.variableCodec.variables.${name}`);
		validateInputRef(
			input,
			`artifact.variableCodec.variables.${name}`,
			inputs,
			limits,
			new Set(),
			0
		);
		variables.set(name, input as ReplicaVariableInputRef);
	}

	for (const [name, definition] of inputs) {
		validateInputDefinition(
			definition,
			`artifact.variableCodec.inputs.${name}`,
			inputs,
			new Set(),
			0
		);
	}
	return { limits, variables, inputs };
}

function validateInputRef(
	value: unknown,
	path: string,
	inputs: ReadonlyMap<string, ReplicaVariableInputDefinition>,
	limits: ReplicaVariableCodecLimits,
	active: Set<object>,
	depth: number
): void {
	checkCodecDepth(depth, path);
	const ref = artifactRecord(value, path);
	withActiveArtifact(ref, active, path, () => {
		switch (ref.kind) {
			case 'scalar':
				exactArtifactKeys(ref, path, [
					'kind',
					'scalar',
					'codec',
					'nullable'
				]);
				assertBoolean(ref.nullable, `${path}.nullable`);
				validateScalarContract(ref.scalar, ref.codec, path);
				return;
			case 'enum':
				exactArtifactKeys(ref, path, ['kind', 'name', 'values', 'nullable']);
				assertGraphqlName(ref.name, `${path}.name`);
				assertBoolean(ref.nullable, `${path}.nullable`);
				validateStringInventory(ref.values, `${path}.values`);
				return;
			case 'input': {
				const hasFilterBaseDepth = Object.prototype.hasOwnProperty.call(
					ref,
					'filterBaseDepth'
				);
				exactArtifactKeys(
					ref,
					path,
					hasFilterBaseDepth
						? ['kind', 'name', 'nullable', 'filterBaseDepth']
						: ['kind', 'name', 'nullable']
				);
				assertGraphqlName(ref.name, `${path}.name`);
				assertBoolean(ref.nullable, `${path}.nullable`);
				const definition = inputs.get(ref.name);
				if (definition === undefined) variableCodecInvalid(`${path}.name`);
				const definitionKind = artifactRecord(
					definition,
					`artifact.variableCodec.inputs.${ref.name}`
				).kind;
				if (definitionKind === 'filter') {
					if (!hasFilterBaseDepth) {
						variableCodecInvalid(`${path}.filterBaseDepth`);
					}
					const filterBaseDepth = validateCodecLimit(
						ref.filterBaseDepth,
						`${path}.filterBaseDepth`
					);
					if (filterBaseDepth > limits.maxDepth) {
						variableCodecInvalid(`${path}.filterBaseDepth`);
					}
				} else if (hasFilterBaseDepth) {
					variableCodecInvalid(`${path}.filterBaseDepth`);
				}
				return;
			}
			case 'list': {
				const hasMaxItems = Object.prototype.hasOwnProperty.call(ref, 'maxItems');
				exactArtifactKeys(
					ref,
					path,
					hasMaxItems
						? ['kind', 'nullable', 'maxItems', 'item']
						: ['kind', 'nullable', 'item']
				);
				assertBoolean(ref.nullable, `${path}.nullable`);
				if (hasMaxItems) validateCodecLimit(ref.maxItems, `${path}.maxItems`);
				validateInputRef(
					ref.item,
					`${path}.item`,
					inputs,
					limits,
					active,
					depth + 1
				);
				return;
			}
			default:
				variableCodecInvalid(`${path}.kind`);
		}
	});
}

function validateInputDefinition(
	value: ReplicaVariableInputDefinition,
	path: string,
	inputs: ReadonlyMap<string, ReplicaVariableInputDefinition>,
	active: Set<object>,
	depth: number
): void {
	checkCodecDepth(depth, path);
	const definition = artifactRecord(value, path);
	withActiveArtifact(definition, active, path, () => {
		switch (definition.kind) {
			case 'filter':
				exactArtifactKeys(definition, path, [
					'kind',
					'model',
					'fields',
					'relationships'
				]);
				assertNonemptyString(definition.model, `${path}.model`);
				validateFilterInputDefinition(
					definition as ReplicaVariableFilterInputDefinition,
					path,
					inputs,
					active,
					depth + 1
				);
				return;
			case 'order':
				exactArtifactKeys(definition, path, [
					'kind',
					'model',
					'fields',
					'values'
				]);
				assertNonemptyString(definition.model, `${path}.model`);
				validateOrderInputDefinition(
					definition as ReplicaVariableOrderInputDefinition,
					path,
					active
				);
				return;
			default:
				variableCodecInvalid(`${path}.kind`);
		}
	});
}

function validateFilterInputDefinition(
	definition: ReplicaVariableFilterInputDefinition,
	path: string,
	inputs: ReadonlyMap<string, ReplicaVariableInputDefinition>,
	active: Set<object>,
	depth: number
): void {
	const fields = artifactArray(definition.fields, `${path}.fields`);
	const relationships = artifactArray(
		definition.relationships,
		`${path}.relationships`
	);
	const names = new Set(['_and', '_or', '_not']);
	for (let index = 0; index < fields.length; index += 1) {
		const fieldPath = `${path}.fields[${index}]`;
		const field = artifactRecord(fields[index], fieldPath, [
			'field',
			'scalar',
			'codec',
			'nullable',
			'operators'
		]);
		withActiveArtifact(field, active, fieldPath, () => {
			assertGraphqlName(field.field, `${fieldPath}.field`);
			if (names.has(field.field)) variableCodecInvalid(`${fieldPath}.field`);
			names.add(field.field);
			assertBoolean(field.nullable, `${fieldPath}.nullable`);
			validateScalarContract(field.scalar, field.codec, fieldPath);
			const operators = artifactArray(field.operators, `${fieldPath}.operators`);
			if (operators.length === 0) variableCodecInvalid(`${fieldPath}.operators`);
			const seen = new Set<string>();
			for (let operator = 0; operator < operators.length; operator += 1) {
				const value = operators[operator];
				if (
					typeof value !== 'string' ||
					!FILTER_OPERATORS.has(value) ||
					seen.has(value)
				) {
					variableCodecInvalid(`${fieldPath}.operators[${operator}]`);
				}
				validateFilterOperatorContract(
					field.scalar as string,
					value,
					`${fieldPath}.operators[${operator}]`
				);
				seen.add(value);
			}
		});
	}
	for (let index = 0; index < relationships.length; index += 1) {
		const relationshipPath = `${path}.relationships[${index}]`;
		const relationship = artifactRecord(
			relationships[index],
			relationshipPath,
			['field', 'target']
		);
		withActiveArtifact(relationship, active, relationshipPath, () => {
			assertGraphqlName(relationship.field, `${relationshipPath}.field`);
			if (names.has(relationship.field)) {
				variableCodecInvalid(`${relationshipPath}.field`);
			}
			names.add(relationship.field);
			const target = artifactRecord(
				relationship.target,
				`${relationshipPath}.target`
			);
			withActiveArtifact(target, active, `${relationshipPath}.target`, () => {
				if (target.kind === 'opaque') {
					exactArtifactKeys(target, `${relationshipPath}.target`, ['kind']);
					return;
				}
				if (target.kind !== 'input') {
					variableCodecInvalid(`${relationshipPath}.target.kind`);
				}
				exactArtifactKeys(target, `${relationshipPath}.target`, ['kind', 'name']);
				assertGraphqlName(target.name, `${relationshipPath}.target.name`);
				const targetDefinition = inputs.get(target.name);
				if (
					targetDefinition === undefined ||
					artifactRecord(
						targetDefinition,
						`artifact.variableCodec.inputs.${target.name}`
					).kind !== 'filter'
				) {
					variableCodecInvalid(`${relationshipPath}.target.name`);
				}
			});
		});
	}
	checkCodecDepth(depth, path);
}

function validateOrderInputDefinition(
	definition: ReplicaVariableOrderInputDefinition,
	path: string,
	active: Set<object>
): void {
	const fields = artifactArray(definition.fields, `${path}.fields`);
	if (fields.length === 0) variableCodecInvalid(`${path}.fields`);
	const names = new Set<string>();
	for (let index = 0; index < fields.length; index += 1) {
		const fieldPath = `${path}.fields[${index}]`;
		const field = artifactRecord(fields[index], fieldPath, [
			'field',
			'scalar',
			'codec',
			'nullable'
		]);
		withActiveArtifact(field, active, fieldPath, () => {
			assertGraphqlName(field.field, `${fieldPath}.field`);
			if (names.has(field.field)) variableCodecInvalid(`${fieldPath}.field`);
			names.add(field.field);
			assertBoolean(field.nullable, `${fieldPath}.nullable`);
			validateScalarContract(field.scalar, field.codec, fieldPath);
		});
	}
	validateStringInventory(definition.values, `${path}.values`);
}

function canonicalizeInputRef(
	input: ReplicaVariableInputRef,
	value: unknown,
	registry: VariableCodecRegistry,
	path: string,
	active: Set<object>,
	depth: number
): ReplicaValue {
	checkValueDepth(depth, path);
	if (input.kind === 'input') {
		const definition = registry.inputs.get(input.name);
		if (definition === undefined) {
			variableCodecInvalid(`artifact.variableCodec.inputs.${input.name}`);
		}
		if (definition.kind === 'filter') {
			if (input.filterBaseDepth === undefined) {
				variableCodecInvalid(`${path}.filterBaseDepth`);
			}
			checkFilterDepth(input.filterBaseDepth, registry.limits, path);
		}
	}
	if (value === null) {
		if (!input.nullable) variableValueInvalid(path, 'non-null value required');
		return null;
	}
	switch (input.kind) {
		case 'scalar':
			return canonicalizeScalar(input.scalar, input.codec, value, path);
		case 'enum':
			if (typeof value !== 'string' || !input.values.includes(value)) {
				variableValueInvalid(path, `expected enum ${input.name}`);
			}
			return value;
		case 'input': {
			const definition = registry.inputs.get(input.name);
			if (definition === undefined) {
				variableCodecInvalid(`artifact.variableCodec.inputs.${input.name}`);
			}
			return canonicalizeInputDefinition(
				definition,
				value,
				registry,
				path,
				active,
				depth + 1,
				input.filterBaseDepth
			);
		}
		case 'list': {
			const values = Array.isArray(value)
				? inputArrayValues(value, path)
				: [value];
			if (input.maxItems !== undefined && values.length > input.maxItems) {
				variableValueInvalid(
					path,
					`list contains ${values.length} items, exceeding maxItems ${input.maxItems}`
				);
			}
			const tracksArray = Array.isArray(value);
			if (tracksArray) beginValue(value, active, path);
			try {
				return Object.freeze(
					values.map((entry, index) => {
						const item = entry === undefined ? null : entry;
						return canonicalizeInputRef(
							input.item,
							item,
							registry,
							`${path}[${index}]`,
							active,
							depth + 1
						);
					})
				);
			} finally {
				if (tracksArray) active.delete(value);
			}
		}
	}
}

function canonicalizeInputDefinition(
	definition: ReplicaVariableInputDefinition,
	value: unknown,
	registry: VariableCodecRegistry,
	path: string,
	active: Set<object>,
	depth: number,
	filterBaseDepth: number | undefined
): ReplicaValue {
	checkValueDepth(depth, path);
	if (definition.kind === 'filter') {
		if (filterBaseDepth === undefined) {
			variableCodecInvalid(`${path}.filterBaseDepth`);
		}
		return canonicalizeFilterInput(
			definition,
			value,
			registry,
			path,
			active,
			depth,
			filterBaseDepth
		);
	}
	return canonicalizeOrderInput(definition, value, path, active, depth);
}

function canonicalizeFilterInput(
	definition: ReplicaVariableFilterInputDefinition,
	value: unknown,
	registry: VariableCodecRegistry,
	path: string,
	active: Set<object>,
	depth: number,
	filterDepth: number
): ReplicaValue {
	checkValueDepth(depth, path);
	checkFilterDepth(filterDepth, registry.limits, path);
	const entries = inputRecordEntries(value, path);
	beginValue(value as object, active, path);
	try {
		const fields = new Map(definition.fields.map((field) => [field.field, field]));
		const relationships = new Map(
			definition.relationships.map((relationship) => [
				relationship.field,
				relationship.target
			])
		);
		const canonical: Array<readonly [string, ReplicaValue]> = [];
		for (const [field, fieldValue] of entries.sort(([left], [right]) =>
			compareCodeUnits(left, right)
		)) {
			if (fieldValue === undefined) continue;
			const fieldPath = `${path}.${field}`;
			if (field === '_and' || field === '_or') {
				if (fieldValue === null) {
					canonical.push([field, null]);
					continue;
				}
				const predicates = Array.isArray(fieldValue)
					? inputArrayValues(fieldValue, fieldPath)
					: [fieldValue];
				if (predicates.length > registry.limits.maxBoolWidth) {
					variableValueInvalid(
						fieldPath,
						`boolean list contains ${predicates.length} items, exceeding maxBoolWidth ${registry.limits.maxBoolWidth}`
					);
				}
				if (Array.isArray(fieldValue)) beginValue(fieldValue, active, fieldPath);
				try {
					canonical.push([
						field,
						Object.freeze(
							predicates.map((predicate, index) => {
								if (predicate === null || predicate === undefined) {
									variableValueInvalid(
										`${fieldPath}[${index}]`,
										'filter list items must be non-null'
									);
								}
								return canonicalizeFilterInput(
									definition,
									predicate,
									registry,
									`${fieldPath}[${index}]`,
									active,
									depth + 1,
									filterDepth + 1
								);
							})
						)
					]);
				} finally {
					if (Array.isArray(fieldValue)) active.delete(fieldValue);
				}
				continue;
			}
			if (field === '_not') {
				const childFilterDepth = filterDepth + 1;
				checkFilterDepth(childFilterDepth, registry.limits, fieldPath);
				canonical.push([
					field,
					fieldValue === null
						? null
						: canonicalizeFilterInput(
								definition,
								fieldValue,
								registry,
								fieldPath,
								active,
								depth + 1,
								childFilterDepth
							)
				]);
				continue;
			}
			const comparison = fields.get(field);
			if (comparison !== undefined) {
				canonical.push([
					field,
					fieldValue === null
						? null
						: canonicalizeFilterComparison(
								comparison,
								fieldValue,
								fieldPath,
								active,
								depth + 1,
								registry.limits
							)
				]);
				continue;
			}
			const relationship = relationships.get(field);
			if (relationship !== undefined) {
				const childFilterDepth = filterDepth + 1;
				checkFilterDepth(childFilterDepth, registry.limits, fieldPath);
				if (fieldValue === null) {
					canonical.push([field, null]);
				} else if (relationship.kind === 'opaque') {
					canonical.push([
						field,
						cloneCanonicalJsonObject(
							fieldValue,
							fieldPath,
							active,
							depth + 1,
							true
						)
					]);
				} else {
					const target = registry.inputs.get(relationship.name);
					if (target?.kind !== 'filter') {
						variableCodecInvalid(
							`artifact.variableCodec.inputs.${relationship.name}`
						);
					}
					canonical.push([
						field,
						canonicalizeFilterInput(
							target,
							fieldValue,
							registry,
							fieldPath,
							active,
							depth + 1,
							childFilterDepth
						)
					]);
				}
				continue;
			}
			variableValueInvalid(fieldPath, 'unknown filter field');
		}
		return freezeRecord(canonical);
	} finally {
		active.delete(value as object);
	}
}

function canonicalizeFilterComparison(
	field: ReplicaVariableFilterInputField,
	value: unknown,
	path: string,
	active: Set<object>,
	depth: number,
	limits: ReplicaVariableCodecLimits
): ReplicaValue {
	checkValueDepth(depth, path);
	const entries = inputRecordEntries(value, path);
	beginValue(value as object, active, path);
	try {
		const allowed = new Set(field.operators);
		const canonical: Array<readonly [string, ReplicaValue]> = [];
		for (const [operator, operand] of entries.sort(([left], [right]) =>
			compareCodeUnits(left, right)
		)) {
			if (operand === undefined) continue;
			const operatorPath = `${path}.${operator}`;
			if (!allowed.has(operator as never)) {
				variableValueInvalid(operatorPath, 'unknown filter operator');
			}
			if (operator === '_in' || operator === '_nin') {
				if (operand === null) {
					variableValueInvalid(operatorPath, 'filter list must be non-null');
				}
				const values = Array.isArray(operand)
					? inputArrayValues(operand, operatorPath)
					: [operand];
				if (values.length > limits.maxInList) {
					variableValueInvalid(
						operatorPath,
						`filter list contains ${values.length} items, exceeding maxInList ${limits.maxInList}`
					);
				}
				if (Array.isArray(operand)) beginValue(operand, active, operatorPath);
				try {
					canonical.push([
						operator,
						Object.freeze(
							values.map((item, index) => {
								if (item === null || item === undefined) {
									variableValueInvalid(
										`${operatorPath}[${index}]`,
										'filter list items must be non-null'
									);
								}
								return canonicalizeScalar(
									field.scalar,
									field.codec,
									item,
									`${operatorPath}[${index}]`
								);
							})
						)
					]);
				} finally {
					if (Array.isArray(operand)) active.delete(operand);
				}
				continue;
			}
			if (operand === null) {
				canonical.push([operator, null]);
				continue;
			}
			if (operator === '_is_null') {
				if (typeof operand !== 'boolean') {
					variableValueInvalid(operatorPath, 'expected boolean or null');
				}
				canonical.push([operator, operand]);
				continue;
			}
			if (operator === '_like' || operator === '_ilike' || operator === '_has_key') {
				if (typeof operand !== 'string') {
					variableValueInvalid(operatorPath, 'expected string or null');
				}
				canonical.push([operator, operand]);
				continue;
			}
			canonical.push([
				operator,
				canonicalizeScalar(field.scalar, field.codec, operand, operatorPath)
			]);
		}
		return freezeRecord(canonical);
	} finally {
		active.delete(value as object);
	}
}

function canonicalizeOrderInput(
	definition: ReplicaVariableOrderInputDefinition,
	value: unknown,
	path: string,
	active: Set<object>,
	depth: number
): ReplicaValue {
	checkValueDepth(depth, path);
	const entries = inputRecordEntries(value, path).filter(
		([, fieldValue]) => fieldValue !== undefined
	);
	if (entries.length !== 1) {
		variableValueInvalid(path, 'order object must contain exactly one field');
	}
	const [field, direction] = entries[0]!;
	if (!definition.fields.some((candidate) => candidate.field === field)) {
		variableValueInvalid(`${path}.${field}`, 'unknown order field');
	}
	if (typeof direction !== 'string' || !definition.values.includes(direction)) {
		variableValueInvalid(`${path}.${field}`, 'unknown order direction');
	}
	beginValue(value as object, active, path);
	try {
		return freezeRecord([[field, direction]]);
	} finally {
		active.delete(value as object);
	}
}

function canonicalizeScalar(
	scalar: string,
	codec: string,
	value: unknown,
	path: string
): ReplicaValue {
	switch (`${scalar}:${codec}`) {
		case 'ID:string':
			if (typeof value === 'string') return value;
			if (typeof value === 'number' && Number.isSafeInteger(value)) {
				return String(Object.is(value, -0) ? 0 : value);
			}
			variableValueInvalid(path, 'ID must be a string or safe integer');
		case 'String:string':
		case 'Timestamptz:string_unvalidated_timestamp':
			if (typeof value !== 'string') variableValueInvalid(path, 'expected string');
			return value;
		case 'Bytea:base64':
			return canonicalizeBase64(value, path);
		case 'Boolean:boolean':
			if (typeof value !== 'boolean') variableValueInvalid(path, 'expected boolean');
			return value;
		case 'Int:int32':
			if (
				typeof value !== 'number' ||
				!Number.isInteger(value) ||
				value < -2_147_483_648 ||
				value > 2_147_483_647
			) {
				variableValueInvalid(path, 'expected signed 32-bit integer');
			}
			return Object.is(value, -0) ? 0 : value;
		case 'Float:float64':
			if (typeof value !== 'number' || !Number.isFinite(value)) {
				variableValueInvalid(path, 'expected finite number');
			}
			return Object.is(value, -0) ? 0 : value;
		case 'BigInt:json_number_precision_limited':
			if (typeof value !== 'number' || !Number.isSafeInteger(value)) {
				variableValueInvalid(path, 'expected safe integer');
			}
			return Object.is(value, -0) ? 0 : value;
		case 'JSON:json':
			return cloneCanonicalJsonValue(value, path, new Set(), 0, false);
		default:
			variableCodecInvalid(path);
	}
}

function canonicalizeBase64(value: unknown, path: string): string {
	if (typeof value !== 'string') variableValueInvalid(path, 'expected base64 string');
	const decode = globalThis.atob;
	const encode = globalThis.btoa;
	if (typeof decode !== 'function' || typeof encode !== 'function') {
		variableValueInvalid(path, 'base64 codec is unavailable in this runtime');
	}
	try {
		const canonical = encode(decode(value));
		if (canonical !== value) {
			variableValueInvalid(path, 'expected canonical standard base64');
		}
		return canonical;
	} catch {
		variableValueInvalid(path, 'expected canonical standard base64');
	}
}

function cloneCanonicalJsonObject(
	value: unknown,
	path: string,
	active: Set<object>,
	depth: number,
	omitUndefined: boolean
): Readonly<Record<string, ReplicaValue>> {
	const entries = inputRecordEntries(value, path);
	checkValueDepth(depth, path);
	beginValue(value as object, active, path);
	try {
		const canonical: Array<readonly [string, ReplicaValue]> = [];
		for (const [key, entry] of entries.sort(([left], [right]) =>
			compareCodeUnits(left, right)
		)) {
			if (entry === undefined && omitUndefined) continue;
			if (entry === undefined) variableValueInvalid(`${path}.${key}`, 'expected JSON value');
			canonical.push([
				key,
				cloneCanonicalJsonValue(
					entry,
					`${path}.${key}`,
					active,
					depth + 1,
					omitUndefined
				)
			]);
		}
		return freezeRecord(canonical);
	} finally {
		active.delete(value as object);
	}
}

function cloneCanonicalJsonValue(
	value: unknown,
	path: string,
	active: Set<object>,
	depth: number,
	omitUndefined: boolean
): ReplicaValue {
	checkValueDepth(depth, path);
	if (value === null || typeof value === 'string' || typeof value === 'boolean') {
		return value;
	}
	if (typeof value === 'number') {
		if (!Number.isFinite(value)) variableValueInvalid(path, 'expected finite JSON number');
		if (Number.isInteger(value) && !Number.isSafeInteger(value)) {
			variableValueInvalid(path, 'expected safe integer or non-integer finite JSON number');
		}
		return Object.is(value, -0) ? 0 : value;
	}
	if (Array.isArray(value)) {
		beginValue(value, active, path);
		try {
			return Object.freeze(
				inputArrayValues(value, path).map((entry, index) => {
					if (entry === undefined) {
						variableValueInvalid(`${path}[${index}]`, 'expected JSON value');
					}
					return cloneCanonicalJsonValue(
						entry,
						`${path}[${index}]`,
						active,
						depth + 1,
						omitUndefined
					);
				})
			);
		} finally {
			active.delete(value);
		}
	}
	return cloneCanonicalJsonObject(value, path, active, depth, omitUndefined);
}

function validateScalarContract(scalar: unknown, codec: unknown, path: string): void {
	if (
		typeof scalar !== 'string' ||
		typeof codec !== 'string' ||
		![
			'ID:string',
			'String:string',
			'Bytea:base64',
			'Timestamptz:string_unvalidated_timestamp',
			'Boolean:boolean',
			'Int:int32',
			'Float:float64',
			'BigInt:json_number_precision_limited',
			'JSON:json'
		].includes(`${scalar}:${codec}`)
	) {
		variableCodecInvalid(`${path}.codec`);
	}
}

function validateFilterOperatorContract(
	scalar: string,
	operator: string,
	path: string
): void {
	if (
		(operator === '_like' || operator === '_ilike') &&
		scalar !== 'String'
	) {
		variableCodecInvalid(path);
	}
	if (
		(operator === '_contains' ||
			operator === '_contained_in' ||
			operator === '_has_key') &&
		scalar !== 'JSON'
	) {
		variableCodecInvalid(path);
	}
}

function validateStringInventory(value: unknown, path: string): void {
	const values = artifactArray(value, path);
	if (values.length === 0) variableCodecInvalid(path);
	const seen = new Set<string>();
	for (let index = 0; index < values.length; index += 1) {
		const entry = values[index];
		if (
			typeof entry !== 'string' ||
			!GRAPHQL_NAME.test(entry) ||
			seen.has(entry)
		) {
			variableCodecInvalid(`${path}[${index}]`);
		}
		seen.add(entry);
	}
}

function artifactRecord(
	value: unknown,
	path: string,
	expectedKeys?: readonly string[]
): Record<string, unknown> {
	const entries = artifactRecordEntries(value, path);
	const record = value as Record<string, unknown>;
	if (expectedKeys !== undefined) exactArtifactKeys(record, path, expectedKeys, entries);
	return record;
}

function artifactRecordEntries(
	value: unknown,
	path: string
): Array<readonly [string, unknown]> {
	return dataRecordEntries(value, path, variableCodecInvalid);
}

function inputRecordEntries(
	value: unknown,
	path: string
): Array<readonly [string, unknown]> {
	return dataRecordEntries(value, path, (invalidPath) =>
		variableValueInvalid(invalidPath, 'expected plain input object')
	);
}

function dataRecordEntries(
	value: unknown,
	path: string,
	fail: (path: string) => never
): Array<readonly [string, unknown]> {
	if (!isPlainRecord(value)) fail(path);
	const entries: Array<readonly [string, unknown]> = [];
	for (const key of Reflect.ownKeys(value)) {
		if (typeof key !== 'string') fail(path);
		const descriptor = Object.getOwnPropertyDescriptor(value, key);
		if (
			descriptor === undefined ||
			!descriptor.enumerable ||
			!('value' in descriptor)
		) {
			fail(`${path}.${key}`);
		}
		entries.push([key, descriptor.value]);
	}
	return entries;
}

function exactArtifactKeys(
	value: Record<string, unknown>,
	path: string,
	expected: readonly string[],
	entries = artifactRecordEntries(value, path)
): void {
	const actual = new Set(entries.map(([key]) => key));
	if (
		actual.size !== expected.length ||
		expected.some((key) => !actual.has(key))
	) {
		variableCodecInvalid(path);
	}
}

function artifactArray(value: unknown, path: string): readonly unknown[] {
	if (!Array.isArray(value)) variableCodecInvalid(path);
	return arrayDataValues(value, path, variableCodecInvalid);
}

function inputArrayValues(value: readonly unknown[], path: string): readonly unknown[] {
	return arrayDataValues(value, path, (invalidPath) =>
		variableValueInvalid(invalidPath, 'expected dense data-only input array')
	);
}

function arrayDataValues(
	value: readonly unknown[],
	path: string,
	fail: (path: string) => never
): readonly unknown[] {
	const result: unknown[] = [];
	const indexes = new Set<string>();
	for (let index = 0; index < value.length; index += 1) {
		const key = String(index);
		const descriptor = Object.getOwnPropertyDescriptor(value, key);
		if (
			descriptor === undefined ||
			!descriptor.enumerable ||
			!('value' in descriptor)
		) {
			fail(`${path}[${index}]`);
		}
		indexes.add(key);
		result.push(descriptor.value);
	}
	for (const key of Reflect.ownKeys(value)) {
		if (key === 'length' || (typeof key === 'string' && indexes.has(key))) continue;
		fail(path);
	}
	return result;
}

function withActiveArtifact(
	value: object,
	active: Set<object>,
	path: string,
	run: () => void
): void {
	if (active.has(value)) variableCodecInvalid(path);
	active.add(value);
	try {
		run();
	} finally {
		active.delete(value);
	}
}

function beginValue(value: object, active: Set<object>, path: string): void {
	if (active.has(value)) variableValueInvalid(path, 'input values must not contain cycles');
	active.add(value);
}

function assertGraphqlName(value: unknown, path: string): asserts value is string {
	if (typeof value !== 'string' || !GRAPHQL_NAME.test(value)) {
		variableCodecInvalid(path);
	}
}

function assertNonemptyString(value: unknown, path: string): asserts value is string {
	if (typeof value !== 'string' || value.length === 0) variableCodecInvalid(path);
}

function assertBoolean(value: unknown, path: string): asserts value is boolean {
	if (typeof value !== 'boolean') variableCodecInvalid(path);
}

function validateCodecLimit(value: unknown, path: string): number {
	if (
		typeof value !== 'number' ||
		!Number.isSafeInteger(value) ||
		value < 0
	) {
		variableCodecInvalid(path);
	}
	return value;
}

function checkCodecDepth(depth: number, path: string): void {
	if (depth > MAX_VARIABLE_CODEC_DEPTH) variableCodecInvalid(path);
}

function checkValueDepth(depth: number, path: string): void {
	if (depth > MAX_VARIABLE_CODEC_DEPTH) {
		variableValueInvalid(path, 'input nesting exceeds the supported depth');
	}
}

function checkFilterDepth(
	depth: number,
	limits: ReplicaVariableCodecLimits,
	path: string
): void {
	if (depth > limits.maxDepth) {
		variableValueInvalid(
			path,
			`filter nesting reaches depth ${depth}, exceeding maxDepth ${limits.maxDepth}`
		);
	}
}

function compareCodeUnits(left: string, right: string): number {
	return left < right ? -1 : left > right ? 1 : 0;
}

function variableCodecInvalid(path: string): never {
	throw new TypeError(`invalid replica variable codec at ${path}`);
}

function variableValueInvalid(path: string, detail: string): never {
	throw new TypeError(`invalid GraphQL operation input at ${path}: ${detail}`);
}

function isPlainRecord(value: unknown): value is Record<string, unknown> {
	if (value === null || typeof value !== 'object' || Array.isArray(value)) {
		return false;
	}
	const prototype = Object.getPrototypeOf(value);
	return prototype === Object.prototype || prototype === null;
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
		const window = effectiveOffsetWindow(argumentsValue, artifact);
		return Object.freeze({
			kind: 'offset' as const,
			offset: window.offset,
			...(window.limit === undefined ? {} : { limit: window.limit }),
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

type OffsetCoverageArtifact = Extract<ReplicaCoverageArtifact, { readonly kind: 'offset' }>;

function effectiveOffsetArguments(
	argumentsValue: Readonly<Record<string, ReplicaValue>>,
	artifact: OffsetCoverageArtifact
): Readonly<Record<string, ReplicaValue>> {
	const window = effectiveOffsetWindow(argumentsValue, artifact);
	const entries = new Map(Object.entries(argumentsValue));
	if (artifact.offsetArgument !== undefined) {
		assertName(artifact.offsetArgument, 'offset pagination argument');
		entries.set(artifact.offsetArgument, window.offset);
	}
	if (artifact.limitArgument !== undefined) {
		assertName(artifact.limitArgument, 'limit pagination argument');
		if (window.limit === undefined) entries.delete(artifact.limitArgument);
		else entries.set(artifact.limitArgument, window.limit);
	}
	return freezeRecord([...entries.entries()]);
}

function effectiveOffsetWindow(
	argumentsValue: Readonly<Record<string, ReplicaValue>>,
	artifact: OffsetCoverageArtifact
): { readonly offset: number; readonly limit?: number } {
	const configuredDefault = coverageBound(artifact.defaultLimit, 'defaultLimit');
	const configuredMax = coverageBound(artifact.maxLimit, 'maxLimit');
	if (
		configuredDefault !== undefined &&
		configuredMax !== undefined &&
		configuredDefault > configuredMax
	) {
		throw new TypeError('offset coverage defaultLimit must not exceed maxLimit');
	}
	const offset =
		serverNonNegativeIntegerArgument(argumentsValue, artifact.offsetArgument) ?? 0;
	const requested = serverNonNegativeIntegerArgument(
		argumentsValue,
		artifact.limitArgument
	);
	const selected = requested ?? configuredDefault;
	const limit =
		selected === undefined || configuredMax === undefined
			? selected
			: Math.min(selected, configuredMax);
	return {
		offset,
		...(limit === undefined ? {} : { limit })
	};
}

/**
 * Distributed's SQL compiler treats an omitted, explicit-null, or negative
 * nullable Int as absent before applying the server default. Other values
 * cannot appear in a successful GraphQL response and remain fail-fast here.
 */
function serverNonNegativeIntegerArgument(
	argumentsValue: Readonly<Record<string, ReplicaValue>>,
	argument: string | undefined
): number | undefined {
	if (
		argument === undefined ||
		!Object.prototype.hasOwnProperty.call(argumentsValue, argument)
	) {
		return undefined;
	}
	const value = argumentsValue[argument];
	if (value === null) return undefined;
	if (!Number.isSafeInteger(value)) {
		throw new TypeError(`pagination argument ${argument} must be an integer or null`);
	}
	if ((value as number) < 0) return undefined;
	return (value as number) === 0 ? 0 : (value as number);
}

function coverageBound(value: number | undefined, field: string): number | undefined {
	if (value === undefined) return undefined;
	if (!Number.isSafeInteger(value) || value < 0) {
		throw new TypeError(`offset coverage ${field} must be a non-negative safe integer`);
	}
	return value;
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
