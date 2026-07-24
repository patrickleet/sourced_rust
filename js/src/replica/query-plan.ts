import type { GraphqlVariables } from '../types.js';
import type { DistributedTrustedPreset } from '../protocol.js';
import type {
	ReplicaArgumentValue,
	ReplicaFilterArtifact,
	ReplicaFilterExpression,
	ReplicaFilterFieldArtifact,
	ReplicaFilterLiteral,
	ReplicaFilterOperand,
	ReplicaFilterOperator,
	ReplicaIndexCoverage,
	ReplicaOrderArtifact,
	ReplicaOrderFieldArtifact,
	ReplicaPaginationArtifact,
	ReplicaPaginationDisposition,
	ReplicaRelationshipArtifact,
	ReplicaRelationshipKeyMapping,
	ReplicaSurfaceTrustedPresetDescriptor,
	ReplicaValue
} from './types.js';
import { resolveReplicaArgumentValue } from './identity.js';

export type ReplicaQueryPlanPath = readonly (string | number)[];

export type ReplicaQueryPlanReasonCode =
	| 'ambiguous_order_entry'
	| 'claim_operand'
	| 'claim_inventory'
	| 'collation_not_portable'
	| 'coverage_mismatch'
	| 'cursor_not_certified'
	| 'delete_changes_offset_window'
	| 'implicit_null_order'
	| 'invalid_argument_value'
	| 'insert_changes_offset_window'
	| 'invalid_artifact'
	| 'invalid_filter_input'
	| 'invalid_filter_value'
	| 'invalid_order_direction'
	| 'invalid_order_input'
	| 'invalid_order_value'
	| 'invalid_pagination_policy'
	| 'missing_field'
	| 'relationship_resolver_required'
	| 'reorder_changes_offset_window'
	| 'server_only_policy'
	| 'sql_null'
	| 'unknown_coverage'
	| 'unknown_field'
	| 'unknown_relationship'
	| 'unsupported_codec'
	| 'unsupported_operator';

export type ReplicaQueryPlanReason = {
	readonly code: ReplicaQueryPlanReasonCode;
	readonly path: ReplicaQueryPlanPath;
	readonly message: string;
};

export type ReplicaFilterEvaluation =
	| { readonly result: 'match' | 'no_match' }
	| {
			readonly result: 'unknown';
			readonly reason: ReplicaQueryPlanReason;
	  };

export type ReplicaRelationshipFilterRequest =
	| {
			readonly source: 'caller';
			readonly relationship: ReplicaRelationshipArtifact;
			readonly predicate: ReplicaValue;
			readonly record: Readonly<Record<string, ReplicaValue>>;
			readonly path: ReplicaQueryPlanPath;
	  }
	| {
			readonly source: 'row_policy';
			readonly relationship: ReplicaRelationshipArtifact;
			readonly predicate: ReplicaFilterExpression;
			readonly record: Readonly<Record<string, ReplicaValue>>;
			readonly path: ReplicaQueryPlanPath;
	  };

export type ReplicaFilterEvaluationOptions = {
	/**
	 * Resolves the server's EXISTS semantics for a relationship predicate.
	 * The resolver is responsible for the target model's row policy as well as
	 * the supplied predicate; omitting it deliberately produces `unknown`.
	 */
	readonly resolveRelationship?: (
		request: ReplicaRelationshipFilterRequest
	) => ReplicaFilterEvaluation;
	/**
	 * Static compiler-owned descriptor union paired with the current
	 * server-derived, cache-scope-bound values. The evaluator never reads
	 * ambient token claims or caller headers.
	 */
	readonly trustedPresets?: {
		readonly descriptors: readonly ReplicaSurfaceTrustedPresetDescriptor[];
		readonly values: readonly DistributedTrustedPreset[];
	};
};

export type ReplicaOrderComparison =
	| { readonly result: 'less' | 'equal' | 'greater' }
	| {
			readonly result: 'unknown';
			readonly reason: ReplicaQueryPlanReason;
	  };

export type ReplicaPaginationChange =
	| { readonly kind: 'insert' }
	| { readonly kind: 'delete' }
	| { readonly kind: 'reorder' }
	| { readonly kind: 'stable_update' };

export type ReplicaPaginationMaintenanceDecision =
	| { readonly decision: 'local' }
	| {
			readonly decision: 'revalidate';
			readonly reason: ReplicaQueryPlanReason;
	  };

type FilterCatalog = {
	readonly fields: ReadonlyMap<string, ReplicaFilterFieldArtifact>;
	readonly relationships: ReadonlyMap<string, ReplicaRelationshipArtifact>;
};

type ResolvedInput =
	| { readonly kind: 'omitted' }
	| { readonly kind: 'value'; readonly value: ReplicaValue }
	| { readonly kind: 'unknown'; readonly reason: ReplicaQueryPlanReason };

type ResolvedOperand =
	| {
			readonly kind: 'value';
			readonly value: ReplicaValue;
			readonly literalKind?: ReplicaFilterLiteral['kind'];
			readonly source: 'literal' | 'trusted_preset';
	  }
	| { readonly kind: 'unknown'; readonly reason: ReplicaQueryPlanReason };

type OrderDirection =
	| 'asc'
	| 'asc_nulls_first'
	| 'asc_nulls_last'
	| 'desc'
	| 'desc_nulls_first'
	| 'desc_nulls_last';

const FILTER_MATCH = Object.freeze({ result: 'match' as const });
const FILTER_NO_MATCH = Object.freeze({ result: 'no_match' as const });
const ORDER_EQUAL = Object.freeze({ result: 'equal' as const });
const PAGINATION_LOCAL = Object.freeze({ decision: 'local' as const });
const UTF8_ENCODER = new TextEncoder();

export function evaluateReplicaFilter(
	artifact: ReplicaFilterArtifact,
	record: Readonly<Record<string, ReplicaValue>>,
	variables: GraphqlVariables = {},
	options: ReplicaFilterEvaluationOptions = {}
): ReplicaFilterEvaluation {
	const catalog = filterCatalog(artifact);
	if ('reason' in catalog) return filterUnknown(catalog.reason);

	const input = resolveInput(artifact.input, variables, ['where']);
	const caller =
		input.kind === 'unknown'
			? filterUnknown(input.reason)
			: input.kind === 'omitted' || input.value === null
				? FILTER_MATCH
				: evaluateCallerWhere(
						input.value,
						record,
						catalog,
						options,
						['where']
					);
	const policy = evaluateRowPolicy(
		artifact,
		record,
		catalog,
		options,
		['rowPolicy']
	);
	return filterAnd([caller, policy]);
}

export function compareReplicaOrder(
	artifact: ReplicaOrderArtifact,
	left: Readonly<Record<string, ReplicaValue>>,
	right: Readonly<Record<string, ReplicaValue>>,
	variables: GraphqlVariables = {}
): ReplicaOrderComparison {
	const fields = new Map<string, ReplicaOrderFieldArtifact>();
	for (const [index, field] of artifact.fields.entries()) {
		if (
			!isName(field.field) ||
			!isScalarCodecPair(field.scalar, field.codec) ||
			typeof field.nullable !== 'boolean' ||
			fields.has(field.field)
		) {
			return orderUnknown(
				reason(
					'invalid_artifact',
					['order', 'fields', index],
					'order fields must have unique names, exact scalar-codec pairs, and nullability'
				)
			);
		}
		fields.set(field.field, field);
	}
	const tieBreakerFields = new Set<string>();
	for (const [index, tieBreaker] of artifact.tieBreakers.entries()) {
		if (
			!isName(tieBreaker.field) ||
			!isScalarCodecPair(tieBreaker.scalar, tieBreaker.codec) ||
			tieBreaker.nullable !== false ||
			tieBreakerFields.has(tieBreaker.field)
		) {
			return orderUnknown(
				reason(
					'invalid_artifact',
					['order', 'tieBreakers', index],
					'order tie-breakers must have unique fields and exact scalar-codec pairs'
				)
			);
		}
		tieBreakerFields.add(tieBreaker.field);
	}

	const input = resolveInput(artifact.input, variables, ['order_by']);
	if (input.kind === 'unknown') return orderUnknown(input.reason);
	const entries: readonly ReplicaValue[] =
		input.kind === 'omitted' || input.value === null
			? []
			: Array.isArray(input.value)
				? input.value
				: isRecord(input.value)
					? [input.value]
					: [];
	if (
		input.kind === 'value' &&
		input.value !== null &&
		!Array.isArray(input.value) &&
		!isRecord(input.value)
	) {
		return orderUnknown(
			reason(
				'invalid_order_input',
				['order_by'],
				'order_by must be an object or a list of objects'
			)
		);
	}

	for (const [index, entry] of entries.entries()) {
		const path: ReplicaQueryPlanPath = ['order_by', index];
		if (!isRecord(entry)) {
			return orderUnknown(
				reason(
					'invalid_order_input',
					path,
					'each order_by entry must be an object'
				)
			);
		}
		const pairs = Object.entries(entry);
		if (pairs.length === 0) continue;
		if (pairs.length > 1) {
			return orderUnknown(
				reason(
					'ambiguous_order_entry',
					path,
					'each order_by entry may declare at most one field'
				)
			);
		}
		const [fieldName, rawDirection] = pairs[0]!;
		const field = fields.get(fieldName);
		if (field === undefined) {
			return orderUnknown(
				reason(
					'unknown_field',
					[...path, fieldName],
					`order field ${fieldName} is not declared by the artifact`
				)
			);
		}
		if (!isOrderDirection(rawDirection)) {
			return orderUnknown(
				reason(
					'invalid_order_direction',
					[...path, fieldName],
					`order direction for ${fieldName} is not supported`
				)
			);
		}
		if (field.nullable && !hasExplicitNullPlacement(rawDirection)) {
			return orderUnknown(
				reason(
					'implicit_null_order',
					[...path, fieldName],
					`nullable order field ${fieldName} requires explicit null placement`
				)
			);
		}
		const compared = compareOrderField(
			field.field,
			field.codec,
			field.nullable,
			rawDirection,
			left,
			right,
			[...path, fieldName]
		);
		if (compared.result !== 'equal') return compared;
	}

	for (const [index, tieBreaker] of artifact.tieBreakers.entries()) {
		const compared = compareOrderField(
			tieBreaker.field,
			tieBreaker.codec,
			false,
			'asc',
			left,
			right,
			['order', 'tieBreakers', index, tieBreaker.field]
		);
		if (compared.result !== 'equal') return compared;
	}
	if (artifact.tieBreakers.length === 0) {
		return orderUnknown(
			reason(
				'invalid_artifact',
				['order', 'tieBreakers'],
				'equal order values require a complete identity tie-breaker'
			)
		);
	}
	return ORDER_EQUAL;
}

export function decideReplicaPaginationMaintenance(
	artifact: ReplicaPaginationArtifact,
	coverage: ReplicaIndexCoverage,
	change: ReplicaPaginationChange
): ReplicaPaginationMaintenanceDecision {
	const invalidPolicy = validatePaginationPolicies(artifact);
	if (invalidPolicy !== undefined) return paginationRevalidate(invalidPolicy);
	if (coverage.kind === 'unknown') {
		return paginationRevalidate(
			reason(
				'unknown_coverage',
				['pagination', 'coverage'],
				'unknown index coverage cannot be maintained locally'
			)
		);
	}
	if (coverage.kind !== artifact.kind) {
		return paginationRevalidate(
			reason(
				'coverage_mismatch',
				['pagination', 'coverage'],
				`artifact coverage ${artifact.kind} does not match index coverage ${coverage.kind}`
			)
		);
	}
	if (artifact.kind === 'cursor') {
		return paginationRevalidate(
			reason(
				'cursor_not_certified',
				['pagination', 'kind'],
				'cursor locality requires a versioned compiler proof IR'
			)
		);
	}

	const policy = policyForChange(artifact, change);
	if (policy === 'local') {
		if (
			artifact.kind !== 'offset' ||
			change.kind === 'stable_update'
		) {
			return PAGINATION_LOCAL;
		}
		if (
			coverage.kind === 'offset' &&
			coverage.offset === 0 &&
			coverage.limit !== undefined &&
			coverage.returned !== undefined &&
			coverage.returned < coverage.limit
		) {
			// A non-full first page proves that the server returned the complete
			// ordered set. The index runtime can therefore apply all optimistic
			// membership/order changes and truncate back to the declared limit.
			return PAGINATION_LOCAL;
		}
	}
	const [code, message]: readonly [ReplicaQueryPlanReasonCode, string] =
		change.kind === 'insert'
			? [
					'insert_changes_offset_window',
					'an insert is local only for a proven non-full first offset page'
				]
			: change.kind === 'delete'
				? [
						'delete_changes_offset_window',
						'a delete is local only for a proven non-full first offset page'
					]
				: change.kind === 'reorder'
					? [
							'reorder_changes_offset_window',
							'a reorder is local only for a proven non-full first offset page'
						]
					: [
							'invalid_pagination_policy',
							'the artifact requires stable updates to revalidate'
						];
	return paginationRevalidate(reason(code, ['pagination', change.kind], message));
}

function evaluateCallerWhere(
	value: ReplicaValue,
	record: Readonly<Record<string, ReplicaValue>>,
	catalog: FilterCatalog,
	options: ReplicaFilterEvaluationOptions,
	path: ReplicaQueryPlanPath
): ReplicaFilterEvaluation {
	if (value === null) return FILTER_MATCH;
	if (!isRecord(value)) {
		return filterUnknown(
			reason(
				'invalid_filter_input',
				path,
				'where predicates must be objects'
			)
		);
	}
	const predicates: ReplicaFilterEvaluation[] = [];
	for (const [key, predicate] of Object.entries(value)) {
		const predicatePath = [...path, key];
		if (key === '_and' || key === '_or') {
			if (predicate === null) continue;
			const items = Array.isArray(predicate)
				? predicate
				: isRecord(predicate)
					? [predicate]
					: undefined;
			if (items === undefined) {
				predicates.push(
					filterUnknown(
						reason(
							'invalid_filter_input',
							predicatePath,
							`${key} must be an object or list of objects`
						)
					)
				);
				continue;
			}
			// The server treats both empty caller lists as no predicate (TRUE).
			if (items.length === 0) continue;
			const evaluated = items.map((item, index) =>
				evaluateCallerWhere(
					item,
					record,
					catalog,
					options,
					[...predicatePath, index]
				)
			);
			predicates.push(key === '_and' ? filterAnd(evaluated) : filterOr(evaluated));
			continue;
		}
		if (key === '_not') {
			if (predicate === null) {
				predicates.push(FILTER_NO_MATCH);
			} else {
				predicates.push(
					filterNot(
						evaluateCallerWhere(
							predicate,
							record,
							catalog,
							options,
							predicatePath
						)
					)
				);
			}
			continue;
		}

		const field = catalog.fields.get(key);
		if (field !== undefined) {
			predicates.push(
				evaluateCallerField(field, predicate, record, predicatePath)
			);
			continue;
		}
		const relationship = catalog.relationships.get(key);
		if (relationship !== undefined) {
			const resolver = options.resolveRelationship;
			predicates.push(
				resolver === undefined
					? filterUnknown(
							reason(
								'relationship_resolver_required',
								predicatePath,
								`relationship predicate ${key} requires an explicit resolver`
							)
						)
					: resolver({
							source: 'caller',
							relationship,
							predicate,
							record,
							path: Object.freeze(predicatePath)
						})
			);
			continue;
		}
		predicates.push(
			filterUnknown(
				reason(
					'unknown_field',
					predicatePath,
					`where field ${key} is not declared by the artifact`
				)
			)
		);
	}
	return filterAnd(predicates);
}

function evaluateCallerField(
	field: ReplicaFilterFieldArtifact,
	value: ReplicaValue,
	record: Readonly<Record<string, ReplicaValue>>,
	path: ReplicaQueryPlanPath
): ReplicaFilterEvaluation {
	if (value === null) return FILTER_MATCH;
	if (!isRecord(value)) {
		return filterUnknown(
			reason(
				'invalid_filter_input',
				path,
				`comparison expression for ${field.field} must be an object`
			)
		);
	}
	const predicates: ReplicaFilterEvaluation[] = [];
	for (const [rawOperator, rhs] of Object.entries(value)) {
		const operatorPath = [...path, rawOperator];
		if (
			!isFilterOperator(rawOperator) ||
			!field.operators.includes(rawOperator)
		) {
			predicates.push(
				filterUnknown(
					reason(
						'unsupported_operator',
						operatorPath,
						`operator ${rawOperator} is not declared for ${field.field}`
					)
				)
			);
			continue;
		}
		const left = recordField(record, field.field, operatorPath);
		if (left.kind === 'unknown') {
			predicates.push(filterUnknown(left.reason));
			continue;
		}
		predicates.push(
			evaluateClientOperator(field, rawOperator, left.value, rhs, operatorPath)
		);
	}
	return filterAnd(predicates);
}

function evaluateClientOperator(
	field: ReplicaFilterFieldArtifact,
	operator: ReplicaFilterOperator,
	left: ReplicaValue,
	rhs: ReplicaValue,
	path: ReplicaQueryPlanPath
): ReplicaFilterEvaluation {
	if (operator === '_is_null') {
		if (rhs !== null && typeof rhs !== 'boolean') {
			return filterUnknown(
				reason(
					'invalid_filter_value',
					path,
					'_is_null must be a boolean or null'
				)
			);
		}
		const wantsNull = rhs === true;
		return (left === null) === wantsNull ? FILTER_MATCH : FILTER_NO_MATCH;
	}
	if (operator === '_in' || operator === '_nin') {
		if (rhs === null) {
			return filterUnknown(
				reason('invalid_filter_value', path, `${operator} requires a list`)
			);
		}
		const values = Array.isArray(rhs) ? rhs : [rhs];
		return evaluateIn(field.codec, left, values, operator === '_nin', path);
	}
	return evaluateComparison(field.codec, operator, left, rhs, path);
}

function evaluateRowPolicy(
	artifact: ReplicaFilterArtifact,
	record: Readonly<Record<string, ReplicaValue>>,
	catalog: FilterCatalog,
	options: ReplicaFilterEvaluationOptions,
	path: ReplicaQueryPlanPath
): ReplicaFilterEvaluation {
	const policy = artifact.rowPolicy;
	if (policy.kind === 'unrestricted') return FILTER_MATCH;
	if (policy.kind === 'server_only') {
		return filterUnknown(
			reason(
				'server_only_policy',
				path,
				'server-only row policy cannot be evaluated in the replica'
			)
		);
	}
	if (policy.kind !== 'predicate' || !isRecord(policy.expression)) {
		return filterUnknown(
			reason('invalid_artifact', path, 'row policy artifact is malformed')
		);
	}
	return evaluatePolicyExpression(
		policy.expression,
		record,
		catalog,
		options,
		[...path, 'expression']
	);
}

function evaluatePolicyExpression(
	expression: ReplicaFilterExpression,
	record: Readonly<Record<string, ReplicaValue>>,
	catalog: FilterCatalog,
	options: ReplicaFilterEvaluationOptions,
	path: ReplicaQueryPlanPath
): ReplicaFilterEvaluation {
	if (expression.kind === 'and' || expression.kind === 'or') {
		if (!Array.isArray(expression.value)) {
			return filterUnknown(
				reason('invalid_artifact', path, 'row-policy boolean value must be a list')
			);
		}
		const evaluated = expression.value.map((item, index) =>
			evaluatePolicyExpression(
				item,
				record,
				catalog,
				options,
				[...path, expression.kind, index]
			)
		);
		// Unlike caller `_or: []`, a row-policy Or([]) is SQL FALSE.
		return expression.kind === 'and' ? filterAnd(evaluated) : filterOr(evaluated);
	}
	if (expression.kind === 'not') {
		return filterNot(
			evaluatePolicyExpression(
				expression.value,
				record,
				catalog,
				options,
				[...path, 'not']
			)
		);
	}
	if (expression.kind === 'rel') {
		const field = expression.value.field;
		const relationshipPath = [...path, 'rel', field];
		const relationship = catalog.relationships.get(field);
		if (relationship === undefined) {
			return filterUnknown(
				reason(
					'unknown_relationship',
					relationshipPath,
					`row policy relationship ${field} is not declared by the artifact`
				)
			);
		}
		const resolver = options.resolveRelationship;
		return resolver === undefined
			? filterUnknown(
					reason(
						'relationship_resolver_required',
						relationshipPath,
						`row policy relationship ${field} requires an explicit resolver`
					)
				)
			: resolver({
					source: 'row_policy',
					relationship,
					predicate: expression.value.predicate,
					record,
					path: Object.freeze(relationshipPath)
				});
	}
	if (
		expression.kind !== 'cmp' &&
		expression.kind !== 'in' &&
		expression.kind !== 'is_null'
	) {
		return filterUnknown(
			reason('invalid_artifact', path, 'row policy expression kind is unsupported')
		);
	}

	const column = expression.value.column;
	const field = catalog.fields.get(column);
	const columnPath = [...path, expression.kind, column];
	if (field === undefined) {
		return filterUnknown(
			reason(
				'unknown_field',
				columnPath,
				`row policy field ${column} is not declared by the artifact`
			)
		);
	}
	const left = recordField(record, column, columnPath);
	if (left.kind === 'unknown') return filterUnknown(left.reason);
	if (expression.kind === 'is_null') {
		if (typeof expression.value.is_null !== 'boolean') {
			return filterUnknown(
				reason(
					'invalid_artifact',
					[...columnPath, 'is_null'],
					'row-policy is_null must be a boolean'
				)
			);
		}
		return (left.value === null) === expression.value.is_null
			? FILTER_MATCH
			: FILTER_NO_MATCH;
	}
	if (expression.kind === 'cmp') {
		const operator = `_${expression.value.op}` as ReplicaFilterOperator;
		if (
			!isFilterOperator(operator) ||
			!isOperatorScalarCompatible(field.scalar, operator)
		) {
			return filterUnknown(
				reason(
					'invalid_artifact',
					[...columnPath, 'op'],
					`row-policy operator ${expression.value.op} cannot target ${field.scalar}`
				)
			);
		}
		const operand = resolveOperand(
			expression.value.rhs,
			field,
			options,
			[...columnPath, 'rhs']
		);
		if (operand.kind === 'unknown') return filterUnknown(operand.reason);
		const literalValidity = validateRowPolicyLiteral(
			field,
			operand,
			[...columnPath, 'rhs'],
			operator
		);
		if (literalValidity !== undefined) return filterUnknown(literalValidity);
		return evaluateComparison(
			field.codec,
			`_${expression.value.op}` as ReplicaFilterOperator,
			left.value,
			operand.value,
			columnPath
		);
	}
	if (expression.kind === 'in') {
		if (
			typeof expression.value.negated !== 'boolean' ||
			!Array.isArray(expression.value.values)
		) {
			return filterUnknown(
				reason(
					'invalid_artifact',
					columnPath,
					'row-policy IN values must be a list and negated must be a boolean'
				)
			);
		}
		const values: ReplicaValue[] = [];
		for (const [index, operand] of expression.value.values.entries()) {
			const operandPath = [...columnPath, 'values', index];
			const resolved = resolveOperand(operand, field, options, operandPath);
			if (resolved.kind === 'unknown') return filterUnknown(resolved.reason);
			const literalValidity = validateRowPolicyLiteral(
				field,
				resolved,
				operandPath
			);
			if (literalValidity !== undefined) return filterUnknown(literalValidity);
			values.push(resolved.value);
		}
		return evaluateIn(
			field.codec,
			left.value,
			values,
			expression.value.negated,
			columnPath
		);
	}
	return filterUnknown(
		reason('invalid_artifact', path, 'row policy expression kind is unsupported')
	);
}

function evaluateComparison(
	codec: string,
	operator: ReplicaFilterOperator,
	left: ReplicaValue,
	right: ReplicaValue,
	path: ReplicaQueryPlanPath
): ReplicaFilterEvaluation {
	if (
		operator === '_like' ||
		operator === '_ilike' ||
		operator === '_contains' ||
		operator === '_contained_in' ||
		operator === '_has_key'
	) {
		return filterUnknown(
			reason(
				'unsupported_operator',
				path,
				`${operator} has no certified cross-dialect replica evaluator`
			)
		);
	}
	if (left === null || right === null) {
		return filterUnknown(
			reason('sql_null', path, 'SQL comparison with null is unknown')
		);
	}
	const validity = validateComparableValues(codec, left, right, path);
	if (validity !== undefined) return filterUnknown(validity);
	if (operator === '_eq' || operator === '_neq') {
		if (codec === 'string' && left !== right) {
			return filterUnknown(
				reason(
					'collation_not_portable',
					path,
					'non-identical strings require an explicit server collation contract'
				)
			);
		}
		const equal = left === right;
		return equal === (operator === '_eq') ? FILTER_MATCH : FILTER_NO_MATCH;
	}
	if (
		operator !== '_gt' &&
		operator !== '_gte' &&
		operator !== '_lt' &&
		operator !== '_lte'
	) {
		return filterUnknown(
			reason('unsupported_operator', path, `operator ${operator} is not portable`)
		);
	}
	if (!isPortableNumericCodec(codec)) {
		return filterUnknown(
			reason(
				'unsupported_operator',
				path,
				`${operator} is portable only for certified numeric codecs`
			)
		);
	}
	const a = left as number;
	const b = right as number;
	const matches =
		operator === '_gt'
			? a > b
			: operator === '_gte'
				? a >= b
				: operator === '_lt'
					? a < b
					: a <= b;
	return matches ? FILTER_MATCH : FILTER_NO_MATCH;
}

function evaluateIn(
	codec: string,
	left: ReplicaValue,
	values: readonly ReplicaValue[],
	negated: boolean,
	path: ReplicaQueryPlanPath
): ReplicaFilterEvaluation {
	// Server compilation special-cases empty lists before evaluating the column.
	if (values.length === 0) return negated ? FILTER_MATCH : FILTER_NO_MATCH;
	if (left === null) {
		return filterUnknown(
			reason('sql_null', path, 'SQL IN comparison with a null column is unknown')
		);
	}
	// GraphQL rejects a type-invalid list before SQL runs. Validate every
	// non-null candidate before allowing a later equality to establish truth.
	// SQL NULL is different: a concrete equality may still dominate UNKNOWN.
	for (const [index, candidate] of values.entries()) {
		if (candidate === null) continue;
		const validity = validateComparableValues(
			codec,
			left,
			candidate,
			[...path, index]
		);
		if (validity !== undefined) return filterUnknown(validity);
	}
	let firstUnknown: ReplicaQueryPlanReason | undefined;
	for (const [index, candidate] of values.entries()) {
		if (candidate === null) {
			firstUnknown ??= reason(
				'sql_null',
				[...path, index],
				'SQL IN list containing null can produce an unknown result'
			);
			continue;
		}
		if (left === candidate) return negated ? FILTER_NO_MATCH : FILTER_MATCH;
		if (codec === 'string') {
			firstUnknown ??= reason(
				'collation_not_portable',
				[...path, index],
				'non-identical strings in SQL IN require an explicit server collation contract'
			);
		}
	}
	if (firstUnknown !== undefined) return filterUnknown(firstUnknown);
	return negated ? FILTER_MATCH : FILTER_NO_MATCH;
}

function compareOrderField(
	field: string,
	codec: string,
	nullable: boolean,
	direction: OrderDirection,
	left: Readonly<Record<string, ReplicaValue>>,
	right: Readonly<Record<string, ReplicaValue>>,
	path: ReplicaQueryPlanPath
): ReplicaOrderComparison {
	const a = recordField(left, field, path);
	if (a.kind === 'unknown') return orderUnknown(a.reason);
	const b = recordField(right, field, path);
	if (b.kind === 'unknown') return orderUnknown(b.reason);
	if ((a.value === null || b.value === null) && !nullable) {
		return orderUnknown(
			reason(
				'invalid_order_value',
				path,
				`non-null order field ${field} contains null`
			)
		);
	}
	if (a.value === null || b.value === null) {
		if (a.value === null && b.value === null) return ORDER_EQUAL;
		if (!hasExplicitNullPlacement(direction)) {
			return orderUnknown(
				reason(
					'implicit_null_order',
					path,
					`nullable order field ${field} requires explicit null placement`
				)
			);
		}
		const nullFirst = direction.endsWith('_nulls_first');
		return orderResult((a.value === null) === nullFirst ? -1 : 1);
	}

	const validity = validateComparableValues(codec, a.value, b.value, path);
	if (validity !== undefined) return orderUnknown(validity);
	let comparison: -1 | 0 | 1;
	if (isPortableNumericCodec(codec)) {
		comparison =
			(a.value as number) < (b.value as number)
				? -1
				: (a.value as number) > (b.value as number)
					? 1
					: 0;
	} else if (codec === 'boolean') {
		comparison =
			a.value === b.value ? 0 : a.value === false && b.value === true ? -1 : 1;
	} else if (codec === 'string') {
		comparison = compareUtf8(
			a.value as string,
			b.value as string
		);
	} else {
		return orderUnknown(
			reason(
				'unsupported_codec',
				path,
				`codec ${codec} has no certified replica comparator`
			)
		);
	}
	if (comparison === 0) return ORDER_EQUAL;
	return orderResult(direction.startsWith('desc') ? invert(comparison) : comparison);
}

function validateComparableValues(
	codec: string,
	left: ReplicaValue,
	right: ReplicaValue,
	path: ReplicaQueryPlanPath
): ReplicaQueryPlanReason | undefined {
	if (codec === 'string') {
		return typeof left === 'string' && typeof right === 'string'
			? undefined
			: reason(
					'invalid_filter_value',
					path,
					'string codec requires string values'
				);
	}
	if (codec === 'boolean') {
		return typeof left === 'boolean' && typeof right === 'boolean'
			? undefined
			: reason(
					'invalid_filter_value',
					path,
					'boolean codec requires boolean values'
				);
	}
	if (codec === 'int32') {
		return isInt32(left) && isInt32(right)
			? undefined
			: reason(
					'invalid_filter_value',
					path,
					'int32 codec requires signed 32-bit integer values'
				);
	}
	if (codec === 'float64') {
		return isFiniteNumber(left) && isFiniteNumber(right)
			? undefined
			: reason(
					'invalid_filter_value',
					path,
					'float64 codec requires finite number values'
				);
	}
	if (codec === 'json_number_precision_limited') {
		return isSafeIntegerNumber(left) && isSafeIntegerNumber(right)
			? undefined
			: reason(
					'invalid_filter_value',
					path,
					'BigInt codec requires JavaScript-safe integer values'
				);
	}
	return reason(
		'unsupported_codec',
		path,
		`codec ${codec} has no certified replica comparison semantics`
	);
}

/**
 * Protocol v2 orders strings by their unsigned UTF-8 bytes. The GraphQL SQL
 * compiler emits the matching binary collation for every textual ORDER BY,
 * making optimistic index maintenance identical on SQLite and PostgreSQL.
 */
function compareUtf8(left: string, right: string): -1 | 0 | 1 {
	if (left === right) return 0;
	const leftBytes = UTF8_ENCODER.encode(left);
	const rightBytes = UTF8_ENCODER.encode(right);
	const length = Math.min(leftBytes.length, rightBytes.length);
	for (let index = 0; index < length; index += 1) {
		const a = leftBytes[index]!;
		const b = rightBytes[index]!;
		if (a !== b) return a < b ? -1 : 1;
	}
	return leftBytes.length < rightBytes.length ? -1 : 1;
}

function validateRowPolicyLiteral(
	field: ReplicaFilterFieldArtifact,
	operand: Extract<ResolvedOperand, { readonly kind: 'value' }>,
	path: ReplicaQueryPlanPath,
	operator?: ReplicaFilterOperator
): ReplicaQueryPlanReason | undefined {
	if (operand.source === 'trusted_preset') return undefined;
	const literalKind = operand.literalKind;
	if (literalKind === undefined) {
		return reason(
			'invalid_artifact',
			path,
			'row-policy literal is missing its tagged kind'
		);
	}
	if (literalKind === 'null') return undefined;
	const expected: readonly ReplicaFilterLiteral['kind'][] | undefined =
		field.scalar === 'ID' ||
		field.scalar === 'String' ||
		field.scalar === 'Timestamptz'
			? ['string']
			: field.scalar === 'Boolean'
				? ['bool']
				: field.scalar === 'Int' || field.scalar === 'BigInt'
					? ['i64']
					: field.scalar === 'Float'
						? ['f64', 'i64']
						: field.scalar === 'JSON'
							? operator === '_has_key'
								? ['string']
								: ['json']
							: undefined;
	if (expected?.includes(literalKind)) return undefined;
	return reason(
		'invalid_artifact',
		path,
		expected === undefined
			? `scalar ${field.scalar} has no non-null row-policy literal encoding`
			: `row-policy literal kind ${literalKind} cannot target ${field.scalar}; expected ${expected.join(' or ')}`
	);
}

function filterCatalog(
	artifact: ReplicaFilterArtifact
):
	| FilterCatalog
	| {
			readonly reason: ReplicaQueryPlanReason;
	  } {
	const fields = new Map<string, ReplicaFilterFieldArtifact>();
	for (const [index, field] of artifact.fields.entries()) {
		if (
			!isName(field.field) ||
			!isScalarCodecPair(field.scalar, field.codec) ||
			typeof field.nullable !== 'boolean' ||
			!Array.isArray(field.operators) ||
			fields.has(field.field)
		) {
			return {
				reason: reason(
					'invalid_artifact',
					['filter', 'fields', index],
					'filter fields must have unique names, exact scalar-codec pairs, nullability, and operators'
				)
			};
		}
		const operators = new Set<ReplicaFilterOperator>();
		for (const [operatorIndex, operator] of field.operators.entries()) {
			if (
				!isFilterOperator(operator) ||
				!isOperatorScalarCompatible(field.scalar, operator) ||
				operators.has(operator)
			) {
				return {
					reason: reason(
						'invalid_artifact',
						['filter', 'fields', index, 'operators', operatorIndex],
						'filter operators must be unique and compatible with the field scalar'
					)
				};
			}
			operators.add(operator);
		}
		fields.set(field.field, field);
	}
	const relationships = new Map<string, ReplicaRelationshipArtifact>();
	for (const [index, value] of artifact.relationships.entries()) {
		const parsed = relationshipArtifact(value, [
			'filter',
			'relationships',
			index
		]);
		if ('reason' in parsed) return parsed;
		const relationship = parsed.value;
		if (relationships.has(relationship.field)) {
			return {
				reason: reason(
					'invalid_artifact',
					['filter', 'relationships', index],
					'filter relationship descriptors must have unique fields'
				)
			};
		}
		relationships.set(relationship.field, relationship);
	}
	return { fields, relationships };
}

function relationshipArtifact(
	value: unknown,
	path: ReplicaQueryPlanPath
):
	| { readonly value: ReplicaRelationshipArtifact }
	| { readonly reason: ReplicaQueryPlanReason } {
	if (
		!isRecord(value) ||
		!isName(value.field) ||
		!isName(value.targetModel) ||
		(value.kind !== 'has_many' &&
			value.kind !== 'belongs_to' &&
			value.kind !== 'many_to_many') ||
		(value.maintenance !== 'local' && value.maintenance !== 'revalidate')
	) {
		return {
			reason: reason(
				'invalid_artifact',
				path,
				'relationship descriptor has invalid field, target, kind, or maintenance'
			)
		};
	}
	const dependencies = uniqueStringList(value.dependencies);
	if (dependencies === undefined || dependencies.length === 0) {
		return {
			reason: reason(
				'invalid_artifact',
				[...path, 'dependencies'],
				'relationship dependencies must be a non-empty unique string list'
			)
		};
	}
	const keyMapping = relationshipKeyMapping(
		value.keyMapping,
		dependencies,
		[...path, 'keyMapping']
	);
	if ('reason' in keyMapping) return keyMapping;
	const localMapping =
		keyMapping.value.kind === 'direct' || keyMapping.value.kind === 'through';
	if (!localMapping && value.maintenance !== 'revalidate') {
		return {
			reason: reason(
				'invalid_artifact',
				[...path, 'maintenance'],
				'opaque and embedded mappings must use revalidation maintenance'
			)
		};
	}
	return {
		value: Object.freeze({
			field: value.field,
			targetModel: value.targetModel,
			kind: value.kind,
			keyMapping: keyMapping.value,
			maintenance: value.maintenance,
			dependencies
		})
	};
}

function relationshipKeyMapping(
	value: unknown,
	dependencies: readonly string[],
	path: ReplicaQueryPlanPath
):
	| { readonly value: ReplicaRelationshipKeyMapping }
	| { readonly reason: ReplicaQueryPlanReason } {
	if (!isRecord(value) || !isName(value.kind)) {
		return {
			reason: reason(
				'invalid_artifact',
				path,
				'relationship key mapping must declare a supported kind'
			)
		};
	}
	if (value.kind === 'embedded') {
		return { value: Object.freeze({ kind: 'embedded' as const }) };
	}
	const local = uniqueStringList(value.local);
	const remote = uniqueStringList(value.remote);
	if (
		local === undefined ||
		remote === undefined ||
		local.length === 0 ||
		local.length !== remote.length
	) {
		return {
			reason: reason(
				'invalid_artifact',
				path,
				'relationship key lists must be unique, non-empty, and equally sized'
			)
		};
	}
	if (value.kind === 'direct') {
		return {
			value: Object.freeze({ kind: 'direct' as const, local, remote })
		};
	}
	if (
		value.kind === 'through' &&
		isName(value.table) &&
		dependencies.includes(value.table) &&
		isName(value.sourceForeignKey) &&
		isName(value.targetForeignKey)
	) {
		return {
			value: Object.freeze({
				kind: 'through' as const,
				local,
				remote,
				table: value.table,
				sourceForeignKey: value.sourceForeignKey,
				targetForeignKey: value.targetForeignKey
			})
		};
	}
	if (
		value.kind === 'through_opaque' &&
		isName(value.dependency) &&
		dependencies.includes(value.dependency)
	) {
		return {
			value: Object.freeze({
				kind: 'through_opaque' as const,
				local,
				remote,
				dependency: value.dependency
			})
		};
	}
	return {
		reason: reason(
			'invalid_artifact',
			path,
			'relationship key mapping is incomplete or references an undeclared dependency'
		)
	};
}

function uniqueStringList(value: unknown): readonly string[] | undefined {
	if (!Array.isArray(value) || value.some((entry) => !isName(entry))) {
		return undefined;
	}
	const values = value as string[];
	if (new Set(values).size !== values.length) return undefined;
	return Object.freeze([...values]);
}

function resolveInput(
	input: ReplicaArgumentValue | undefined,
	variables: GraphqlVariables,
	path: ReplicaQueryPlanPath
): ResolvedInput {
	if (input === undefined) return { kind: 'omitted' };
	try {
		const value = resolveReplicaArgumentValue(input, variables);
		return value === undefined
			? { kind: 'omitted' }
			: { kind: 'value', value };
	} catch (error) {
		return {
			kind: 'unknown',
			reason: reason(
				'invalid_argument_value',
				path,
				error instanceof Error
					? error.message
					: 'plan input could not be resolved'
			)
		};
	}
}

function resolveOperand(
	operand: ReplicaFilterOperand,
	field: ReplicaFilterFieldArtifact,
	options: ReplicaFilterEvaluationOptions,
	path: ReplicaQueryPlanPath
): ResolvedOperand {
	if (operand.kind === 'claim') {
		return resolveTrustedPresetOperand(
			operand.value.header,
			field,
			options,
			path
		);
	}
	if (operand.kind !== 'lit' || !isRecord(operand.value)) {
		return {
			kind: 'unknown',
			reason: reason('invalid_artifact', path, 'row-policy operand is malformed')
		};
	}
	const literal = operand.value;
	if (literal.kind === 'null') {
		return {
			kind: 'value',
			value: null,
			literalKind: literal.kind,
			source: 'literal'
		};
	}
	if (
		literal.kind === 'string' &&
		typeof literal.value === 'string'
	) {
		return {
			kind: 'value',
			value: literal.value,
			literalKind: literal.kind,
			source: 'literal'
		};
	}
	if (literal.kind === 'bool' && typeof literal.value === 'boolean') {
		return {
			kind: 'value',
			value: literal.value,
			literalKind: literal.kind,
			source: 'literal'
		};
	}
	if (
		(literal.kind === 'i64' && Number.isSafeInteger(literal.value)) ||
		(literal.kind === 'f64' && isFiniteNumber(literal.value))
	) {
		return {
			kind: 'value',
			value: literal.value,
			literalKind: literal.kind,
			source: 'literal'
		};
	}
	if (literal.kind === 'json' && isReplicaValue(literal.value)) {
		return {
			kind: 'value',
			value: literal.value,
			literalKind: literal.kind,
			source: 'literal'
		};
	}
	return {
		kind: 'unknown',
		reason: reason(
			'invalid_artifact',
			path,
			'row-policy literal is not a portable JSON value'
		)
	};
}

function resolveTrustedPresetOperand(
	name: string,
	field: ReplicaFilterFieldArtifact,
	options: ReplicaFilterEvaluationOptions,
	path: ReplicaQueryPlanPath
): ResolvedOperand {
	const inventory = options.trustedPresets;
	if (inventory === undefined) {
		return {
			kind: 'unknown',
			reason: reason(
				'claim_operand',
				path,
				`claim ${name} has no authoritative scope-bound preset inventory`
			)
		};
	}
	const descriptors = new Map<string, ReplicaSurfaceTrustedPresetDescriptor>();
	for (const [index, descriptor] of inventory.descriptors.entries()) {
		if (
			typeof descriptor?.name !== 'string' ||
			descriptor.name.length === 0 ||
			typeof descriptor.codec !== 'string' ||
			descriptors.has(descriptor.name)
		) {
			return {
				kind: 'unknown',
				reason: reason(
					'claim_inventory',
					[...path, 'descriptors', index],
					'trusted preset descriptor inventory is malformed or duplicated'
				)
			};
		}
		descriptors.set(descriptor.name, descriptor);
	}
	const values = new Map<string, DistributedTrustedPreset>();
	for (const [index, preset] of inventory.values.entries()) {
		if (
			typeof preset?.name !== 'string' ||
			preset.name.length === 0 ||
			typeof preset.codec !== 'string' ||
			values.has(preset.name)
		) {
			return {
				kind: 'unknown',
				reason: reason(
					'claim_inventory',
					[...path, 'values', index],
					'trusted preset value inventory is malformed or duplicated'
				)
			};
		}
		values.set(preset.name, preset);
	}
	if (
		descriptors.size !== values.size ||
		[...descriptors].some(
			([presetName, descriptor]) =>
				values.get(presetName)?.codec !== descriptor.codec
		)
	) {
		return {
			kind: 'unknown',
			reason: reason(
				'claim_inventory',
				path,
				'trusted preset descriptors and scope-bound values do not form an exact union'
			)
		};
	}
	const descriptor = descriptors.get(name);
	const preset = values.get(name);
	if (descriptor === undefined || preset === undefined) {
		return {
			kind: 'unknown',
			reason: reason(
				'claim_operand',
				path,
				`claim ${name} is absent from the selected client surface`
			)
		};
	}
	if (descriptor.codec !== field.codec) {
		return {
			kind: 'unknown',
			reason: reason(
				'claim_operand',
				path,
				`claim ${name} codec ${descriptor.codec} cannot target ${field.field}:${field.codec}`
			)
		};
	}
	if (!isReplicaValue(preset.value)) {
		return {
			kind: 'unknown',
			reason: reason(
				'claim_inventory',
				path,
				`claim ${name} is not a portable replica value`
			)
		};
	}
	return {
		kind: 'value',
		value: preset.value,
		source: 'trusted_preset'
	};
}

function recordField(
	record: Readonly<Record<string, ReplicaValue>>,
	field: string,
	path: ReplicaQueryPlanPath
):
	| { readonly kind: 'value'; readonly value: ReplicaValue }
	| { readonly kind: 'unknown'; readonly reason: ReplicaQueryPlanReason } {
	if (!Object.prototype.hasOwnProperty.call(record, field)) {
		return {
			kind: 'unknown',
			reason: reason(
				'missing_field',
				path,
				`record does not contain canonical field ${field}`
			)
		};
	}
	return { kind: 'value', value: record[field]! };
}

function filterAnd(
	values: readonly ReplicaFilterEvaluation[]
): ReplicaFilterEvaluation {
	let firstUnknown: ReplicaQueryPlanReason | undefined;
	for (const value of values) {
		if (value.result === 'no_match') return FILTER_NO_MATCH;
		if (value.result === 'unknown') firstUnknown ??= value.reason;
	}
	return firstUnknown === undefined ? FILTER_MATCH : filterUnknown(firstUnknown);
}

function filterOr(
	values: readonly ReplicaFilterEvaluation[]
): ReplicaFilterEvaluation {
	let firstUnknown: ReplicaQueryPlanReason | undefined;
	for (const value of values) {
		if (value.result === 'match') return FILTER_MATCH;
		if (value.result === 'unknown') firstUnknown ??= value.reason;
	}
	return firstUnknown === undefined ? FILTER_NO_MATCH : filterUnknown(firstUnknown);
}

function filterNot(value: ReplicaFilterEvaluation): ReplicaFilterEvaluation {
	if (value.result === 'unknown') return value;
	return value.result === 'match' ? FILTER_NO_MATCH : FILTER_MATCH;
}

function filterUnknown(reasonValue: ReplicaQueryPlanReason): ReplicaFilterEvaluation {
	return Object.freeze({ result: 'unknown' as const, reason: reasonValue });
}

function orderUnknown(reasonValue: ReplicaQueryPlanReason): ReplicaOrderComparison {
	return Object.freeze({ result: 'unknown' as const, reason: reasonValue });
}

function orderResult(comparison: -1 | 1): ReplicaOrderComparison {
	return Object.freeze({
		result: comparison < 0 ? ('less' as const) : ('greater' as const)
	});
}

function paginationRevalidate(
	reasonValue: ReplicaQueryPlanReason
): ReplicaPaginationMaintenanceDecision {
	return Object.freeze({
		decision: 'revalidate' as const,
		reason: reasonValue
	});
}

function reason(
	code: ReplicaQueryPlanReasonCode,
	path: ReplicaQueryPlanPath,
	message: string
): ReplicaQueryPlanReason {
	return Object.freeze({
		code,
		path: Object.freeze([...path]),
		message
	});
}

function validatePaginationPolicies(
	artifact: ReplicaPaginationArtifact
): ReplicaQueryPlanReason | undefined {
	if (
		artifact.kind !== 'complete' &&
		artifact.kind !== 'offset' &&
		artifact.kind !== 'cursor'
	) {
		return reason(
			'invalid_pagination_policy',
			['pagination', 'kind'],
			'pagination kind must be complete, offset, or cursor'
		);
	}
	const values: readonly (readonly [
		keyof Pick<
			ReplicaPaginationArtifact,
			'insert' | 'delete' | 'reorder' | 'stableUpdate'
		>,
		ReplicaPaginationDisposition
	])[] = [
		['insert', artifact.insert],
		['delete', artifact.delete],
		['reorder', artifact.reorder],
		['stableUpdate', artifact.stableUpdate]
	];
	for (const [field, value] of values) {
		if (value !== 'local' && value !== 'revalidate') {
			return reason(
				'invalid_pagination_policy',
				['pagination', field],
				`pagination policy ${field} must be local or revalidate`
			);
		}
	}
	const exact =
		artifact.kind === 'complete'
			? values.every(([, value]) => value === 'local')
			: artifact.kind === 'offset'
				? artifact.stableUpdate === 'local'
				: true;
	return exact
		? undefined
		: reason(
				'invalid_pagination_policy',
				['pagination'],
				`${artifact.kind} pagination policies do not match the conservative compiler contract`
			);
}

function policyForChange(
	artifact: ReplicaPaginationArtifact,
	change: ReplicaPaginationChange
): ReplicaPaginationDisposition {
	return change.kind === 'stable_update'
		? artifact.stableUpdate
		: artifact[change.kind];
}

function isFilterOperator(value: string): value is ReplicaFilterOperator {
	return (
		value === '_eq' ||
		value === '_neq' ||
		value === '_gt' ||
		value === '_gte' ||
		value === '_lt' ||
		value === '_lte' ||
		value === '_in' ||
		value === '_nin' ||
		value === '_is_null' ||
		value === '_like' ||
		value === '_ilike' ||
		value === '_contains' ||
		value === '_contained_in' ||
		value === '_has_key'
	);
}

function isScalarCodecPair(scalar: unknown, codec: unknown): boolean {
	if (typeof scalar !== 'string' || typeof codec !== 'string') return false;
	return (
		`${scalar}:${codec}` === 'ID:string' ||
		`${scalar}:${codec}` === 'String:string' ||
		`${scalar}:${codec}` === 'Bytea:base64' ||
		`${scalar}:${codec}` ===
			'Timestamptz:string_unvalidated_timestamp' ||
		`${scalar}:${codec}` === 'Boolean:boolean' ||
		`${scalar}:${codec}` === 'Int:int32' ||
		`${scalar}:${codec}` === 'Float:float64' ||
		`${scalar}:${codec}` ===
			'BigInt:json_number_precision_limited' ||
		`${scalar}:${codec}` === 'JSON:json'
	);
}

function isOperatorScalarCompatible(
	scalar: string,
	operator: ReplicaFilterOperator
): boolean {
	if (operator === '_like' || operator === '_ilike') {
		return scalar === 'String';
	}
	if (
		operator === '_contains' ||
		operator === '_contained_in' ||
		operator === '_has_key'
	) {
		return scalar === 'JSON';
	}
	return true;
}

function isPortableNumericCodec(codec: string): boolean {
	return (
		codec === 'int32' ||
		codec === 'float64' ||
		codec === 'json_number_precision_limited'
	);
}

function isSafeIntegerNumber(value: ReplicaValue): value is number {
	return typeof value === 'number' && Number.isSafeInteger(value);
}

function isOrderDirection(value: ReplicaValue): value is OrderDirection {
	return (
		value === 'asc' ||
		value === 'asc_nulls_first' ||
		value === 'asc_nulls_last' ||
		value === 'desc' ||
		value === 'desc_nulls_first' ||
		value === 'desc_nulls_last'
	);
}

function hasExplicitNullPlacement(direction: OrderDirection): boolean {
	return direction.includes('_nulls_');
}

function invert(value: -1 | 1): -1 | 1 {
	return value === -1 ? 1 : -1;
}

function isInt32(value: ReplicaValue): value is number {
	return (
		Number.isInteger(value) &&
		(value as number) >= -2_147_483_648 &&
		(value as number) <= 2_147_483_647
	);
}

function isFiniteNumber(value: unknown): value is number {
	return typeof value === 'number' && Number.isFinite(value);
}

function isName(value: unknown): value is string {
	return typeof value === 'string' && value.length > 0;
}

function isRecord(value: ReplicaValue | unknown): value is {
	readonly [key: string]: ReplicaValue;
} {
	return value !== null && typeof value === 'object' && !Array.isArray(value);
}

function isReplicaValue(value: unknown, ancestors = new Set<object>()): value is ReplicaValue {
	if (
		value === null ||
		typeof value === 'string' ||
		typeof value === 'boolean' ||
		isFiniteNumber(value)
	) {
		return true;
	}
	if (typeof value !== 'object' || ancestors.has(value)) return false;
	ancestors.add(value);
	const valid = Array.isArray(value)
		? value.every((entry) => isReplicaValue(entry, ancestors))
		: (Object.getPrototypeOf(value) === Object.prototype ||
				Object.getPrototypeOf(value) === null) &&
			Object.values(value).every((entry) => isReplicaValue(entry, ancestors));
	ancestors.delete(value);
	return valid;
}
