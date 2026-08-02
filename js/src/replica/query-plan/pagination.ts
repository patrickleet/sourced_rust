import type {
	ReplicaIndexCoverage,
	ReplicaPaginationArtifact,
	ReplicaPaginationDisposition
} from '../types.js';
import { PAGINATION_LOCAL } from './constants.js';
import type {
	ReplicaPaginationChange,
	ReplicaPaginationMaintenanceDecision,
	ReplicaQueryPlanReason,
	ReplicaQueryPlanReasonCode
} from './types.js';
import { reason } from './util.js';

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
			coverage.limit !== undefined
		) {
			/*
			 * First-page (offset 0) locality:
			 * - insert: always local when a limit is known. The maintainer
			 *   re-sorts known members and truncates to the window, so a full
			 *   lobby page still shows an optimistic chat/todo at the front.
			 * - delete/reorder: only when the page is non-full (complete ordered
			 *   set). A full page with hasNext cannot prove the next boundary
			 *   after a delete or order change.
			 */
			if (change.kind === 'insert') {
				return PAGINATION_LOCAL;
			}
			if (
				coverage.returned !== undefined &&
				coverage.returned < coverage.limit
			) {
				return PAGINATION_LOCAL;
			}
		}
	}
	const [code, message]: readonly [ReplicaQueryPlanReasonCode, string] =
		change.kind === 'insert'
			? [
					'insert_changes_offset_window',
					'an insert is local only for the first offset page (offset 0) with a known limit'
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

export function paginationRevalidate(
	reasonValue: ReplicaQueryPlanReason
): ReplicaPaginationMaintenanceDecision {
	return Object.freeze({
		decision: 'revalidate' as const,
		reason: reasonValue
	});
}

export function validatePaginationPolicies(
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

export function policyForChange(
	artifact: ReplicaPaginationArtifact,
	change: ReplicaPaginationChange
): ReplicaPaginationDisposition {
	return change.kind === 'stable_update'
		? artifact.stableUpdate
		: artifact[change.kind];
}
