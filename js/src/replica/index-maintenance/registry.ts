import type { GraphqlVariables } from '../../types.js';
import type { DistributedTrustedPreset } from '../../protocol.js';
import { canonicalizeOperationVariables } from '../identity.js';
import { compareCodeUnits } from '../../lib/compare-code-units.js';
import type { ReplicaOperationArtifact } from '../types.js';
import type {
	ReplicaIndexMaintenanceDecision,
	ReplicaIndexMaintenanceRegistry,
	ReplicaIndexMaintenanceSnapshot,
	ReplicaIndexPlanRegistration,
	ReplicaIndexSemanticLayer
} from './types.js';
import {
	applyLayers,
	canonicalValue,
	compilePlans,
	evaluatePlan,
	instantiatePlans,
	planAffected,
	reason,
	stale,
	validateIndexes,
	type RegisteredPlan
} from './engine.js';

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
