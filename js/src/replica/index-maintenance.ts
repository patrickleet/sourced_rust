import type { GraphqlVariables } from '../types.js';
import type { DistributedTrustedPreset } from '../protocol.js';
import {
	canonicalizeOperationVariables,
	replicaIndexKey,
	resolveArguments
} from './identity.js';
import {
	compareReplicaOrder,
	decideReplicaPaginationMaintenance,
	evaluateReplicaFilter
} from './query-plan.js';
import type {
	ReplicaFilterArtifact,
	ReplicaCoverageArtifact,
	ReplicaIndexCoverage,
	ReplicaIndexRecordChange,
	ReplicaIndexRelationshipChange,
	ReplicaIndexSemanticChange,
	ReplicaObjectBranch,
	ReplicaOperationArtifact,
	ReplicaOrderArtifact,
	ReplicaPaginationArtifact,
	ReplicaRelationshipArtifact,
	ReplicaRootSelection,
	ReplicaSurfaceTrustedPresetDescriptor,
	ReplicaValue
} from './types.js';

import { compareCodeUnits } from '../lib/compare-code-units.js';

export type {
	ReplicaIndexDependencyChange,
	ReplicaIndexRecordChange,
	ReplicaIndexRelationshipChange,
	ReplicaIndexSemanticChange
} from './types.js';

export type ReplicaIndexMaintenanceRecord = {
	readonly key: string;
	readonly model: string;
	readonly fields: Readonly<Record<string, ReplicaValue>>;
};

export type ReplicaIndexMaintenanceIndex = {
	readonly key: string;
	readonly records: readonly string[];
	readonly complete: boolean;
	readonly metadata: {
		readonly parent?: string;
		readonly field: string;
		readonly arguments: Readonly<Record<string, ReplicaValue>>;
		readonly coverage: ReplicaIndexCoverage;
		readonly dependencies: readonly string[];
		readonly staleReason?: string;
	};
};

export type ReplicaIndexSemanticLayer = {
	readonly id: string;
	readonly changes: readonly ReplicaIndexSemanticChange[];
};

export type ReplicaIndexMaintenanceSnapshot = {
	readonly records: readonly ReplicaIndexMaintenanceRecord[];
	readonly indexes: readonly ReplicaIndexMaintenanceIndex[];
};

export type ReplicaIndexMaintenanceReasonCode =
	| 'aggregate_dependency_changed'
	| 'conflicting_index_plan'
	| 'dependency_changed'
	| 'duplicate_index_record'
	| 'index_incomplete'
	| 'invalid_index_metadata'
	| 'missing_index_record'
	| 'missing_order_plan'
	| 'missing_parent_record'
	| 'non_unique_order'
	| 'relationship_dependency_missing'
	| 'relationship_maintenance_revalidate'
	| 'relationship_mapping_unknown'
	| 'unsupported_cardinality'
	| import('./query-plan.js').ReplicaQueryPlanReasonCode;

export type ReplicaIndexMaintenanceReason = {
	readonly code: ReplicaIndexMaintenanceReasonCode;
	readonly path: readonly (string | number)[];
	readonly message: string;
};

export type ReplicaIndexMaintenanceDecision =
	| {
			readonly kind: 'write';
			readonly indexKey: string;
			readonly records: readonly string[];
			readonly complete: true;
	  }
	| {
			readonly kind: 'stale';
			readonly indexKey: string;
			readonly reason: ReplicaIndexMaintenanceReason;
	  }
	| {
			readonly kind: 'unchanged';
			readonly indexKey: string;
	  };

export type ReplicaIndexPlanRegistration = {
	readonly id: string;
	dispose(): void;
};

export interface ReplicaIndexMaintenanceRegistry {
	registerOperation<TData, TVariables extends GraphqlVariables>(
		artifact: ReplicaOperationArtifact<TData, TVariables>,
		variables: TVariables
	): ReplicaIndexPlanRegistration;
	evaluate(
		snapshot: ReplicaIndexMaintenanceSnapshot,
		layers: readonly ReplicaIndexSemanticLayer[],
		trustedPresets?: readonly DistributedTrustedPreset[]
	): readonly ReplicaIndexMaintenanceDecision[];
	clear(): void;
}

type PlanKind = 'entity' | 'relationship' | 'snapshot';

type RegisteredPlan = {
	readonly registration: string;
	readonly kind: PlanKind;
	readonly exactIndexKey?: string;
	readonly parentModel?: string;
	readonly targetModel?: string;
	readonly field: string;
	readonly arguments: Readonly<Record<string, ReplicaValue>>;
	readonly dependencies: readonly string[];
	readonly cardinality: 'one' | 'many';
	readonly filter?: ReplicaFilterArtifact;
	readonly order?: ReplicaOrderArtifact;
	readonly pagination?: ReplicaPaginationArtifact;
	readonly coverage?: ReplicaCoverageArtifact;
	readonly relationship?: ReplicaRelationshipArtifact;
	readonly variables: GraphqlVariables;
	readonly trustedPresets: readonly ReplicaSurfaceTrustedPresetDescriptor[];
	readonly path: readonly (string | number)[];
	readonly signature: string;
};

type MutableRecord = {
	readonly key: string;
	readonly model: string;
	fields: Record<string, ReplicaValue>;
};

type AppliedChanges = {
	readonly baseRecords: ReadonlyMap<string, MutableRecord>;
	readonly records: Map<string, MutableRecord>;
	readonly recordChanges: readonly ReplicaIndexRecordChange[];
	readonly relationshipChanges: readonly ReplicaIndexRelationshipChange[];
	readonly invalidatedDependencies: ReadonlySet<string>;
	readonly allDependencies: ReadonlySet<string>;
};

class PurposeBuiltIndexMaintenanceRegistry
	implements ReplicaIndexMaintenanceRegistry
{
	readonly #plans = new Map<string, readonly RegisteredPlan[]>();
	#nextRegistration = 0;

	registerOperation<TData, TVariables extends GraphqlVariables>(
		artifact: ReplicaOperationArtifact<TData, TVariables>,
		variables: TVariables
	): ReplicaIndexPlanRegistration {
		const canonicalVariables = canonicalizeOperationVariables(artifact, variables);
		const id = `${artifact.id}:${canonicalValue(canonicalVariables)}:${++this.#nextRegistration}`;
		this.#plans.set(id, compilePlans(id, artifact, canonicalVariables));
		let active = true;
		return Object.freeze({
			id,
			dispose: () => {
				if (!active) return;
				active = false;
				this.#plans.delete(id);
			}
		});
	}

	evaluate(
		snapshot: ReplicaIndexMaintenanceSnapshot,
		layers: readonly ReplicaIndexSemanticLayer[],
		trustedPresets: readonly DistributedTrustedPreset[] = Object.freeze([])
	): readonly ReplicaIndexMaintenanceDecision[] {
		const applied = applyLayers(snapshot.records, layers);
		const indexes = validateIndexes(snapshot.indexes);
		const plansByIndex = instantiatePlans(
			[...this.#plans.values()].flat(),
			indexes
		);
		const decisions: ReplicaIndexMaintenanceDecision[] = [];
		for (const [indexKey, plans] of [...plansByIndex].sort(([left], [right]) =>
			compareCodeUnits(left, right)
		)) {
			const index = indexes.get(indexKey);
			if (index === undefined) continue;
			const affected = plans.filter((plan) => planAffected(plan, index, applied));
			if (affected.length === 0) continue;
			const signatures = new Set(affected.map(({ signature }) => signature));
			if (signatures.size !== 1) {
				decisions.push(
					stale(
						indexKey,
						reason(
							'conflicting_index_plan',
							['indexes', indexKey],
							'multiple active artifacts disagree about one semantic index plan'
						)
					)
				);
				continue;
			}
			decisions.push(
				evaluatePlan(affected[0]!, index, applied, trustedPresets)
			);
		}
		return Object.freeze(decisions);
	}

	clear(): void {
		this.#plans.clear();
	}
}

export function createReplicaIndexMaintenanceRegistry(): ReplicaIndexMaintenanceRegistry {
	return new PurposeBuiltIndexMaintenanceRegistry();
}

export function formatReplicaIndexStaleReason(
	reasonValue: ReplicaIndexMaintenanceReason
): string {
	const path =
		reasonValue.path.length === 0
			? ''
			: `:${reasonValue.path.map(String).join('.')}`;
	return `query-plan:${reasonValue.code}${path}`;
}

function compilePlans(
	registration: string,
	artifact: ReplicaOperationArtifact<unknown, GraphqlVariables>,
	variables: GraphqlVariables
): readonly RegisteredPlan[] {
	const plans: RegisteredPlan[] = [];
	const trustedPresets = artifact.protocol.trustedPresets;
	for (const [index, root] of artifact.roots.entries()) {
		const path: readonly (string | number)[] = ['roots', index, root.field];
		const targetModel = selectionModel(root.selection);
		const kind: PlanKind =
			targetModel === undefined ? 'snapshot' : 'entity';
		const argumentsValue = resolveArguments(
			root.arguments,
			variables,
			root.coverage
		);
		plans.push(
			plan({
				registration,
				kind,
				exactIndexKey: replicaIndexKey({
					field: root.field,
					arguments: argumentsValue
				}),
				targetModel,
				field: root.field,
				arguments: argumentsValue,
				dependencies: root.dependencies,
				cardinality: root.cardinality,
				filter: root.filter,
				order: root.order,
				pagination: root.pagination,
				coverage: root.coverage,
				variables,
				trustedPresets,
				path
			})
		);
		compileSelectionPlans(
			plans,
			registration,
			root.selection,
			targetModel,
			variables,
			trustedPresets,
			path
		);
	}
	return Object.freeze(plans);
}

function compileSelectionPlans(
	output: RegisteredPlan[],
	registration: string,
	selection: ReplicaRootSelection['selection'],
	parentModel: string | undefined,
	variables: GraphqlVariables,
	trustedPresets: readonly ReplicaSurfaceTrustedPresetDescriptor[],
	path: readonly (string | number)[]
): void {
	for (const [index, member] of selection.members.entries()) {
		if (member.kind !== 'branch') continue;
		compileBranchPlan(
			output,
			registration,
			member,
			parentModel,
			variables,
			trustedPresets,
			[...path, 'members', index, member.field]
		);
	}
}

function compileBranchPlan(
	output: RegisteredPlan[],
	registration: string,
	branch: ReplicaObjectBranch,
	parentModel: string | undefined,
	variables: GraphqlVariables,
	trustedPresets: readonly ReplicaSurfaceTrustedPresetDescriptor[],
	path: readonly (string | number)[]
): void {
	const argumentsValue = resolveArguments(
		branch.arguments,
		variables,
		branch.coverage
	);
	const targetModel = selectionModel(branch.selection);
	output.push(
		plan({
			registration,
			kind: branch.semantic === 'relationship' ? 'relationship' : 'snapshot',
			parentModel,
			targetModel,
			field: branch.field,
			arguments: argumentsValue,
			dependencies: branch.dependencies,
			cardinality: branch.cardinality,
			filter: branch.filter,
			order: branch.order,
			pagination: branch.pagination,
			coverage: branch.coverage,
			relationship: branch.relationship,
			variables,
			trustedPresets,
			path
		})
	);
	compileSelectionPlans(
		output,
		registration,
		branch.selection,
		targetModel ?? parentModel,
		variables,
		trustedPresets,
		path
	);
}

function plan(
	value: Omit<RegisteredPlan, 'signature'>
): RegisteredPlan {
	const signature = canonicalValue({
		kind: value.kind,
		parentModel: value.parentModel ?? null,
		targetModel: value.targetModel ?? null,
		field: value.field,
		arguments: value.arguments,
		dependencies: [...value.dependencies].sort(compareCodeUnits),
		cardinality: value.cardinality,
		filter: value.filter ?? null,
		order: value.order ?? null,
		pagination: value.pagination ?? null,
		coverage: value.coverage ?? null,
		relationship: value.relationship ?? null,
		trustedPresets: value.trustedPresets
	});
	return Object.freeze({
		...value,
		arguments: cloneRecord(value.arguments),
		dependencies: Object.freeze([...new Set(value.dependencies)].sort(compareCodeUnits)),
		trustedPresets: Object.freeze(
			value.trustedPresets.map((descriptor) =>
				Object.freeze({ name: descriptor.name, codec: descriptor.codec })
			)
		),
		path: Object.freeze([...value.path]),
		signature
	});
}

function selectionModel(
	selection: ReplicaRootSelection['selection']
): string | undefined {
	return selection.storage.kind === 'normalized'
		? selection.storage.model
		: undefined;
}

function instantiatePlans(
	plans: readonly RegisteredPlan[],
	indexes: ReadonlyMap<string, ReplicaIndexMaintenanceIndex>
): Map<string, RegisteredPlan[]> {
	const result = new Map<string, RegisteredPlan[]>();
	for (const value of plans) {
		if (value.exactIndexKey !== undefined) {
			if (indexes.has(value.exactIndexKey)) {
				appendPlan(result, value.exactIndexKey, value);
			}
			continue;
		}
		for (const index of indexes.values()) {
			const parent = index.metadata.parent;
			if (
				parent === undefined ||
				index.metadata.field !== value.field ||
				replicaIndexKey({
					parent,
					field: value.field,
					arguments: value.arguments
				}) !== index.key ||
				(value.parentModel !== undefined &&
					!recordKeyMatchesModel(parent, value.parentModel))
			) {
				continue;
			}
			appendPlan(result, index.key, value);
		}
	}
	return result;
}

function appendPlan(
	output: Map<string, RegisteredPlan[]>,
	key: string,
	value: RegisteredPlan
): void {
	const current = output.get(key) ?? [];
	if (
		current.some(
			(candidate) =>
				candidate.registration === value.registration &&
				candidate.signature === value.signature
		)
	) {
		return;
	}
	current.push(value);
	output.set(key, current);
}

function applyLayers(
	base: readonly ReplicaIndexMaintenanceRecord[],
	layers: readonly ReplicaIndexSemanticLayer[]
): AppliedChanges {
	const records = new Map<string, MutableRecord>();
	for (const input of base) {
		validateName(input.key, 'record key');
		validateName(input.model, 'record model');
		if (records.has(input.key)) {
			throw new TypeError(`duplicate maintenance record: ${input.key}`);
		}
		records.set(input.key, {
			key: input.key,
			model: input.model,
			fields: { ...input.fields }
		});
	}
	const baseRecords = new Map(
		[...records].map(([key, record]) => [
			key,
			{
				key: record.key,
				model: record.model,
				fields: { ...record.fields }
			}
		])
	);
	const recordChanges: ReplicaIndexRecordChange[] = [];
	const relationshipChanges: ReplicaIndexRelationshipChange[] = [];
	const invalidatedDependencies = new Set<string>();
	const allDependencies = new Set<string>();
	const layerIds = new Set<string>();
	for (const layer of layers) {
		validateName(layer.id, 'semantic layer id');
		if (layerIds.has(layer.id)) {
			throw new TypeError(`duplicate semantic layer: ${layer.id}`);
		}
		layerIds.add(layer.id);
		for (const change of layer.changes) {
			if (change.kind === 'invalidate') {
				for (const dependency of validateDependencies(change.dependencies)) {
					invalidatedDependencies.add(dependency);
					allDependencies.add(dependency);
				}
				continue;
			}
			if (isRelationshipChange(change)) {
				validateName(change.sourceModel, 'relationship source model');
				validateName(change.field, 'relationship field');
				validateName(change.targetModel, 'relationship target model');
				validateName(change.sourceKey, 'relationship source key');
				validateName(change.targetKey, 'relationship target key');
				for (const dependency of validateDependencies(change.dependencies)) {
					allDependencies.add(dependency);
				}
				relationshipChanges.push(change);
				continue;
			}
			validateName(change.model, 'record change model');
			validateName(change.key, 'record change key');
			for (const dependency of validateDependencies(change.dependencies ?? [])) {
				allDependencies.add(dependency);
			}
			recordChanges.push(change);
			if (change.kind === 'delete') {
				records.delete(change.key);
				continue;
			}
			const current = records.get(change.key);
			if (current !== undefined && current.model !== change.model) {
				throw new TypeError(`record model changed for ${change.key}`);
			}
			records.set(change.key, {
				key: change.key,
				model: change.model,
				fields: { ...(current?.fields ?? {}), ...change.fields }
			});
		}
	}
	return {
		baseRecords,
		records,
		recordChanges: Object.freeze(recordChanges),
		relationshipChanges: Object.freeze(relationshipChanges),
		invalidatedDependencies,
		allDependencies
	};
}

function validateIndexes(
	input: readonly ReplicaIndexMaintenanceIndex[]
): Map<string, ReplicaIndexMaintenanceIndex> {
	const indexes = new Map<string, ReplicaIndexMaintenanceIndex>();
	for (const index of input) {
		validateName(index.key, 'index key');
		if (indexes.has(index.key)) {
			throw new TypeError(`duplicate maintenance index: ${index.key}`);
		}
		indexes.set(index.key, index);
	}
	return indexes;
}

function planAffected(
	planValue: RegisteredPlan,
	index: ReplicaIndexMaintenanceIndex,
	changes: AppliedChanges
): boolean {
	if (
		planValue.kind === 'snapshot' &&
		intersects(planValue.dependencies, changes.allDependencies)
	) {
		return true;
	}
	if (
		intersects(planValue.dependencies, changes.invalidatedDependencies)
	) {
		return true;
	}
	if (
		planValue.targetModel !== undefined &&
		changes.recordChanges.some(({ model }) => model === planValue.targetModel)
	) {
		return true;
	}
	if (
		planValue.kind === 'relationship' &&
		planValue.parentModel !== undefined &&
		changes.recordChanges.some((change) =>
			parentRecordChangeAffectsRelationship(planValue, index, change)
		)
	) {
		return true;
	}
	return changes.relationshipChanges.some((change) =>
		relationshipChangeMatches(planValue, index, change)
	);
}

function evaluatePlan(
	planValue: RegisteredPlan,
	index: ReplicaIndexMaintenanceIndex,
	changes: AppliedChanges,
	trustedPresets: readonly DistributedTrustedPreset[]
): ReplicaIndexMaintenanceDecision {
	if (planValue.kind === 'snapshot') {
		return stale(
			index.key,
			reason(
				'aggregate_dependency_changed',
				planValue.path,
				'an aggregate or embedded query snapshot dependency changed'
			)
		);
	}
	if (intersects(planValue.dependencies, changes.invalidatedDependencies)) {
		return stale(
			index.key,
			reason(
				'dependency_changed',
				planValue.path,
				'a declared dependency changed without locally provable row evidence'
			)
		);
	}
	/*
	 * Freshness and structure are independent. Revalidation deliberately marks
	 * a complete index stale while retaining its last authoritative membership.
	 * Pending semantic layers must keep rebasing over that membership; the
	 * derived write preserves the stale metadata until the network result lands.
	 */
	if (!index.complete) {
		return stale(
			index.key,
			reason(
				'index_incomplete',
				['indexes', index.key],
				'only a structurally complete index can be maintained locally'
			)
		);
	}
	if (planValue.cardinality !== 'many') {
		const targetChanges = changes.recordChanges.filter(
			(change) => change.model === planValue.targetModel
		);
		if (
			changes.relationshipChanges.length === 0 &&
			targetChanges.length > 0 &&
			targetChanges.every(
				(change) =>
					change.kind === 'upsert' && index.records.includes(change.key)
			)
		) {
			// Updating fields on the already-selected singular identity cannot
			// change that index's membership. The sparse record dependency will
			// still publish selected field changes to the UI.
			return unchanged(index.key);
		}
		return stale(
			index.key,
			reason(
				'unsupported_cardinality',
				planValue.path,
				'local query-index maintenance is limited to collection fields'
			)
		);
	}
	if (
		index.metadata.field !== planValue.field ||
		!sameValue(index.metadata.arguments, planValue.arguments)
	) {
		return stale(
			index.key,
			reason(
				'invalid_index_metadata',
				['indexes', index.key],
				'stored index metadata does not match its compiler plan'
			)
		);
	}
	const duplicate = firstDuplicate(index.records);
	if (duplicate !== undefined) {
		return stale(
			index.key,
			reason(
				'duplicate_index_record',
				['indexes', index.key],
				`stored index contains duplicate record ${duplicate}`
			)
		);
	}
	if (
		planValue.kind === 'relationship' &&
		planValue.relationship?.maintenance !== 'local'
	) {
		return stale(
			index.key,
			reason(
				'relationship_maintenance_revalidate',
				planValue.path,
				'the compiler marked this relationship as revalidation-only'
			)
		);
	}
	return maintainEntityIndex(planValue, index, changes, trustedPresets);
}

function maintainEntityIndex(
	planValue: RegisteredPlan,
	index: ReplicaIndexMaintenanceIndex,
	changes: AppliedChanges,
	trustedPresets: readonly DistributedTrustedPreset[]
): ReplicaIndexMaintenanceDecision {
	const current = [...index.records];
	const members = new Set(current);
	const candidates = new Set<string>();
	for (const change of changes.recordChanges) {
		if (change.model === planValue.targetModel) candidates.add(change.key);
	}
	const relationshipOverrides = new Map<string, boolean>();
	if (planValue.kind === 'relationship') {
		const parentChanged = changes.recordChanges.some((change) =>
			parentRecordChangeAffectsRelationship(planValue, index, change)
		);
		if (parentChanged) {
			return stale(
				index.key,
				reason(
					'relationship_mapping_unknown',
					planValue.path,
					'a parent-key dependency changed and sparse target coverage cannot prove the new relationship'
				)
			);
		}
		for (const change of changes.relationshipChanges) {
			if (!relationshipChangeMatches(planValue, index, change)) continue;
			const missing = planValue.relationship!.dependencies.filter(
				(dependency) => !change.dependencies.includes(dependency)
			);
			if (missing.length > 0) {
				return stale(
					index.key,
					reason(
						'relationship_dependency_missing',
						planValue.path,
						`relationship change omits declared dependencies: ${missing.join(', ')}`
					)
				);
			}
			candidates.add(change.targetKey);
			relationshipOverrides.set(change.targetKey, change.kind === 'link');
		}
	}

	let inserted = false;
	let removed = false;
	for (const key of candidates) {
		const record = changes.records.get(key);
		let related = true;
		if (planValue.kind === 'relationship') {
			const override = relationshipOverrides.get(key);
			if (override !== undefined) {
				related = override;
			} else if (members.has(key)) {
				related = true;
			} else {
				const relation = relationshipMembership(
					planValue,
					index,
					record,
					changes.records
				);
				if ('reason' in relation) return stale(index.key, relation.reason);
				related = relation.related;
			}
		}
		if (record === undefined || !related) {
			if (members.delete(key)) removed = true;
			continue;
		}
		const filter = evaluateMembership(planValue, record, trustedPresets);
		if ('reason' in filter) return stale(index.key, filter.reason);
		if (filter.matches) {
			if (!members.has(key)) {
				members.add(key);
				inserted = true;
			}
		} else if (members.delete(key)) {
			removed = true;
		}
	}

	let activeOrderChanged = false;
	if (planValue.order !== undefined) {
		for (const key of candidates) {
			if (!members.has(key)) continue;
			const before = changes.baseRecords.get(key);
			const after = changes.records.get(key);
			if (before === undefined || after === undefined) continue;
			const compared = compareReplicaOrder(
				planValue.order,
				before.fields,
				after.fields,
				planValue.variables
			);
			if (compared.result === 'unknown') {
				return stale(index.key, compared.reason);
			}
			if (compared.result !== 'equal') {
				activeOrderChanged = true;
				break;
			}
		}
	}
	let records = current.filter((key) => members.has(key));
	for (const key of members) {
		if (!records.includes(key)) records.push(key);
	}
	const needsOrder = inserted || activeOrderChanged;
	if (needsOrder) {
		if (planValue.order === undefined && inserted) {
			return stale(
				index.key,
				reason(
					'missing_order_plan',
					planValue.path,
					'an inserted record cannot be positioned without a compiler order plan'
				)
			);
		}
		if (planValue.order !== undefined) {
			const sorted = sortRecords(
				records,
				planValue.order,
				changes.records,
				planValue.variables,
				planValue.path
			);
			if ('reason' in sorted) return stale(index.key, sorted.reason);
			records = [...sorted.records];
		}
	}
	const reordered =
		!inserted &&
		!removed &&
		(activeOrderChanged || !sameStringList(records, current));
	const changeKind =
		inserted
			? 'insert'
			: removed
				? 'delete'
				: reordered
					? 'reorder'
					: 'stable_update';
	if (planValue.pagination === undefined) {
		if (
			changeKind === 'stable_update' &&
			sameStringList(records, current)
		) {
			// An uncertified collection can still prove a membership/order no-op;
			// that stable update needs no index write or revalidation.
			return unchanged(index.key);
		}
		return stale(
			index.key,
			reason(
				'invalid_artifact',
				planValue.path,
				'collection plan is missing its pagination maintenance contract'
			)
		);
	}
	const coverageReason = certifyOffsetCoverage(
		planValue,
		index,
		current.length,
		changeKind
	);
	if (coverageReason !== undefined) {
		return stale(index.key, coverageReason);
	}
	const pagination = decideReplicaPaginationMaintenance(
		planValue.pagination,
		index.metadata.coverage,
		{ kind: changeKind }
	);
	if (pagination.decision === 'revalidate') {
		return stale(index.key, pagination.reason);
	}
	if (
		planValue.pagination.kind === 'offset' &&
		index.metadata.coverage.kind === 'offset' &&
		index.metadata.coverage.offset === 0 &&
		index.metadata.coverage.limit !== undefined &&
		records.length > index.metadata.coverage.limit
	) {
		// Locality above is possible only because the base first page was
		// non-full, so this is the complete optimistic ordered set. Preserve the
		// operation's exact window when stacked optimistic inserts cross its
		// limit.
		records = records.slice(0, index.metadata.coverage.limit);
	}
	return sameStringList(records, current)
		? unchanged(index.key)
		: write(index.key, records);
}

function certifyOffsetCoverage(
	planValue: RegisteredPlan,
	index: ReplicaIndexMaintenanceIndex,
	confirmedRecords: number,
	changeKind: 'insert' | 'delete' | 'reorder' | 'stable_update'
): ReplicaIndexMaintenanceReason | undefined {
	if (
		planValue.pagination?.kind !== 'offset' ||
		changeKind === 'stable_update'
	) {
		return undefined;
	}
	const artifact = planValue.coverage;
	const coverage = index.metadata.coverage;
	if (artifact?.kind !== 'offset' || coverage.kind !== 'offset') {
		return reason(
			'invalid_index_metadata',
			['indexes', index.key, 'coverage'],
			'offset pagination locality requires its exact compiler coverage contract'
		);
	}
	const expectedOffset =
		artifact.offsetArgument === undefined
			? 0
			: planValue.arguments[artifact.offsetArgument];
	const configuredLimit =
		artifact.limitArgument === undefined
			? artifact.defaultLimit
			: planValue.arguments[artifact.limitArgument];
	const expectedLimit =
		typeof configuredLimit === 'number' &&
		typeof artifact.maxLimit === 'number'
			? Math.min(configuredLimit, artifact.maxLimit)
			: configuredLimit;
	if (
		typeof expectedOffset !== 'number' ||
		!Number.isSafeInteger(expectedOffset) ||
		expectedOffset < 0 ||
		(typeof expectedLimit !== 'number' &&
			expectedLimit !== undefined) ||
		(typeof expectedLimit === 'number' &&
			(!Number.isSafeInteger(expectedLimit) || expectedLimit < 0)) ||
		coverage.offset !== expectedOffset ||
		coverage.limit !== expectedLimit ||
		coverage.returned !== confirmedRecords ||
		coverage.hasNext === true
	) {
		return reason(
			'invalid_index_metadata',
			['indexes', index.key, 'coverage'],
			'offset coverage does not match the request, confirmed index size, or observed page boundary'
		);
	}
	return undefined;
}

function parentRecordChangeAffectsRelationship(
	planValue: RegisteredPlan,
	index: ReplicaIndexMaintenanceIndex,
	change: ReplicaIndexRecordChange
): boolean {
	if (
		change.model !== planValue.parentModel ||
		change.key !== index.metadata.parent
	) {
		return false;
	}
	if (change.kind === 'delete') return true;
	const relationship = planValue.relationship;
	if (relationship === undefined) {
		// A forged relationship branch without its compiler-owned mapping must
		// be evaluated and rejected instead of silently ignoring a parent change.
		return true;
	}
	const mapping = relationship.keyMapping;
	const mappingFields =
		mapping.kind === 'embedded' ? Object.freeze([]) : mapping.local;
	if (
		mappingFields.some((field) =>
			Object.prototype.hasOwnProperty.call(change.fields, field)
		)
	) {
		return true;
	}
	const dependencies = new Set(change.dependencies ?? []);
	return relationship.dependencies.some((dependency) =>
		dependencies.has(dependency)
	);
}

function relationshipMembership(
	planValue: RegisteredPlan,
	index: ReplicaIndexMaintenanceIndex,
	target: MutableRecord | undefined,
	records: ReadonlyMap<string, MutableRecord>
):
	| { readonly related: boolean }
	| { readonly reason: ReplicaIndexMaintenanceReason } {
	if (target === undefined) return { related: false };
	const relationship = planValue.relationship;
	const parentKey = index.metadata.parent;
	if (relationship === undefined || parentKey === undefined) {
		return {
			reason: reason(
				'relationship_mapping_unknown',
				planValue.path,
				'relationship plan or parent identity is unavailable'
			)
		};
	}
	if (relationship.keyMapping.kind !== 'direct') {
		return {
			reason: reason(
				'relationship_mapping_unknown',
				planValue.path,
				'a non-member through/opaque relationship requires explicit link evidence'
			)
		};
	}
	const parent = records.get(parentKey);
	if (parent === undefined) {
		return {
			reason: reason(
				'missing_parent_record',
				planValue.path,
				'relationship parent record is absent'
			)
		};
	}
	for (let indexValue = 0; indexValue < relationship.keyMapping.local.length; indexValue += 1) {
		const local = relationship.keyMapping.local[indexValue]!;
		const remote = relationship.keyMapping.remote[indexValue]!;
		if (
			!Object.prototype.hasOwnProperty.call(parent.fields, local) ||
			!Object.prototype.hasOwnProperty.call(target.fields, remote)
		) {
			return {
				reason: reason(
					'missing_field',
					[...planValue.path, 'relationship', local, remote],
					'relationship key fields are not completely present'
				)
			};
		}
		if (!sameValue(parent.fields[local], target.fields[remote])) {
			return { related: false };
		}
	}
	return { related: true };
}

function evaluateMembership(
	planValue: RegisteredPlan,
	record: MutableRecord,
	trustedPresets: readonly DistributedTrustedPreset[]
):
	| { readonly matches: boolean }
	| { readonly reason: ReplicaIndexMaintenanceReason } {
	if (planValue.filter === undefined) return { matches: true };
	const evaluated = evaluateReplicaFilter(
		planValue.filter,
		record.fields,
		planValue.variables,
		{
			trustedPresets: {
				descriptors: planValue.trustedPresets,
				values: trustedPresets
			}
		}
	);
	return evaluated.result === 'unknown'
		? { reason: evaluated.reason }
		: { matches: evaluated.result === 'match' };
}

function sortRecords(
	keys: readonly string[],
	order: ReplicaOrderArtifact,
	records: ReadonlyMap<string, MutableRecord>,
	variables: GraphqlVariables,
	path: readonly (string | number)[]
):
	| { readonly records: readonly string[] }
	| { readonly reason: ReplicaIndexMaintenanceReason } {
	let unknown: ReplicaIndexMaintenanceReason | undefined;
	const sorted = [...keys].sort((leftKey, rightKey) => {
		if (unknown !== undefined || leftKey === rightKey) return 0;
		const left = records.get(leftKey);
		const right = records.get(rightKey);
		if (left === undefined || right === undefined) {
			unknown = reason(
				'missing_index_record',
				path,
				`ordered index references missing record ${left === undefined ? leftKey : rightKey}`
			);
			return 0;
		}
		const compared = compareReplicaOrder(
			order,
			left.fields,
			right.fields,
			variables
		);
		if (compared.result === 'unknown') {
			unknown = compared.reason;
			return 0;
		}
		if (compared.result === 'equal') {
			unknown = reason(
				'non_unique_order',
				path,
				`distinct records ${leftKey} and ${rightKey} have no deterministic order`
			);
			return 0;
		}
		return compared.result === 'less' ? -1 : 1;
	});
	return unknown === undefined
		? { records: Object.freeze(sorted) }
		: { reason: unknown };
}

function relationshipChangeMatches(
	planValue: RegisteredPlan,
	index: ReplicaIndexMaintenanceIndex,
	change: ReplicaIndexRelationshipChange
): boolean {
	return (
		planValue.kind === 'relationship' &&
		planValue.parentModel === change.sourceModel &&
		planValue.field === change.field &&
		planValue.targetModel === change.targetModel &&
		index.metadata.parent === change.sourceKey
	);
}

function isRelationshipChange(
	change: ReplicaIndexSemanticChange
): change is ReplicaIndexRelationshipChange {
	return change.kind === 'link' || change.kind === 'unlink';
}

function validateDependencies(value: readonly string[]): readonly string[] {
	if (!Array.isArray(value)) throw new TypeError('dependencies must be an array');
	const seen = new Set<string>();
	for (const dependency of value) {
		validateName(dependency, 'dependency');
		if (seen.has(dependency)) {
			throw new TypeError(`duplicate dependency: ${dependency}`);
		}
		seen.add(dependency);
	}
	return value;
}

function intersects(
	left: readonly string[],
	right: ReadonlySet<string>
): boolean {
	return left.some((value) => right.has(value));
}

function recordKeyMatchesModel(key: string, model: string): boolean {
	return key.startsWith(`record:${encodeURIComponent(model)}:`);
}

function firstDuplicate(values: readonly string[]): string | undefined {
	const seen = new Set<string>();
	for (const value of values) {
		if (seen.has(value)) return value;
		seen.add(value);
	}
	return undefined;
}

function write(
	indexKey: string,
	records: readonly string[]
): ReplicaIndexMaintenanceDecision {
	return Object.freeze({
		kind: 'write' as const,
		indexKey,
		records: Object.freeze([...records]),
		complete: true as const
	});
}

function stale(
	indexKey: string,
	reasonValue: ReplicaIndexMaintenanceReason
): ReplicaIndexMaintenanceDecision {
	return Object.freeze({
		kind: 'stale' as const,
		indexKey,
		reason: reasonValue
	});
}

function unchanged(indexKey: string): ReplicaIndexMaintenanceDecision {
	return Object.freeze({ kind: 'unchanged' as const, indexKey });
}

function reason(
	code: ReplicaIndexMaintenanceReasonCode,
	path: readonly (string | number)[],
	message: string
): ReplicaIndexMaintenanceReason {
	return Object.freeze({
		code,
		path: Object.freeze([...path]),
		message
	});
}

function sameStringList(
	left: readonly string[],
	right: readonly string[]
): boolean {
	return (
		left.length === right.length &&
		left.every((value, index) => value === right[index])
	);
}

function sameValue(left: unknown, right: unknown): boolean {
	return canonicalValue(left) === canonicalValue(right);
}

function cloneRecord(
	value: Readonly<Record<string, ReplicaValue>>
): Readonly<Record<string, ReplicaValue>> {
	return Object.freeze(
		Object.fromEntries(
			Object.entries(value)
				.sort(([left], [right]) => compareCodeUnits(left, right))
				.map(([key, entry]) => [key, cloneValue(entry)])
		)
	);
}

function cloneValue(value: ReplicaValue): ReplicaValue {
	if (Array.isArray(value)) return Object.freeze(value.map(cloneValue));
	if (value !== null && typeof value === 'object') {
		return cloneRecord(value as Readonly<Record<string, ReplicaValue>>);
	}
	return value;
}

function canonicalValue(value: unknown): string {
	if (value === null) return 'null';
	if (typeof value === 'string') return JSON.stringify(value);
	if (typeof value === 'boolean') return value ? 'true' : 'false';
	if (typeof value === 'number') {
		if (!Number.isFinite(value)) throw new TypeError('canonical values must be finite');
		return Object.is(value, -0) ? '0' : JSON.stringify(value);
	}
	if (Array.isArray(value)) {
		return `[${value.map(canonicalValue).join(',')}]`;
	}
	if (typeof value === 'object') {
		const entries = Object.entries(value as Readonly<Record<string, unknown>>)
			.filter(([, entry]) => entry !== undefined)
			.sort(([left], [right]) => compareCodeUnits(left, right));
		return `{${entries
			.map(([key, entry]) => `${JSON.stringify(key)}:${canonicalValue(entry)}`)
			.join(',')}}`;
	}
	throw new TypeError('canonical values must be JSON-compatible');
}

function validateName(value: string, description: string): void {
	if (typeof value !== 'string' || value.length === 0) {
		throw new TypeError(`${description} must be a non-empty string`);
	}
}
