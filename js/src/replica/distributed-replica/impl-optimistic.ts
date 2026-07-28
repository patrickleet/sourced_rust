import type { CacheEngine } from '../../internal/cache-engine.js';
import {
	DistributedProtocolError,
	type DistributedCommandMetadata,
	type DistributedProjectionObservation
} from '../../protocol.js';
import type {
	ReplicaDiagnosticEventInput,
	ReplicaDiagnosticLayerInput
} from '../diagnostics.js';
import type { ReplicaIndexSemanticChange } from '../index-maintenance.js';
import type {
	ReplicaBaseWriter,
	ReplicaOptimisticWriter
} from '../types.js';
import { protocolInvalid } from './clocks.js';
import { baseWriter } from './helpers.js';
import {
	assertReplicaOptimisticLayerId,
	captureReplicaOptimisticUpdate,
	cloneOptimisticReceipt,
	diagnosticReceiptCounts,
	expectationKey,
	optimisticReceiptState,
	replayReplicaOptimisticUpdate,
	sameReceipt
} from './optimistic.js';
import type { OptimisticReceiptState } from './types.js';

/**
 * Host for optimistic layer lifecycle and receipt planning.
 */
export type OptimisticHost = {
	readonly engine: CacheEngine;
	readonly optimisticReceipts: Map<string, OptimisticReceiptState>;
	readonly diagnosticLayers: Map<string, ReplicaDiagnosticLayerInput> | undefined;
	readonly diagnostics: { readonly enabled: boolean };
	getDiagnosticLayerSequence(): number;
	setDiagnosticLayerSequence(value: number): void;
	diagnosticEvent(event: ReplicaDiagnosticEventInput): void;
	syncDiagnostics(): void;
	retireDiagnosticLayer(
		id: string,
		action: 'retired' | 'rejected',
		receiptState: 'projected' | 'rejected',
		receipt?: OptimisticReceiptState
	): void;
};

export function createOptimisticLayerOn(
	host: OptimisticHost,
	id: string,
	update: (writer: ReplicaOptimisticWriter) => void,
	semanticChanges: readonly ReplicaIndexSemanticChange[] = Object.freeze([])
): void {
	assertReplicaOptimisticLayerId(id);
	if (host.engine.optimisticLayerState(id) !== undefined) {
		throw new Error(`optimistic layer already exists: ${id}`);
	}
	if (typeof update !== 'function') {
		throw new TypeError('optimistic layer update must be a function');
	}
	const captured = captureReplicaOptimisticUpdate(
		id,
		update,
		semanticChanges
	);
	host.engine.createOptimisticLayer(
		id,
		(writer) => replayReplicaOptimisticUpdate(writer, captured.operations),
		captured.context
	);
	if (host.diagnosticLayers !== undefined) {
		const recordChanges = captured.operations.filter(
			(operation) =>
				operation.kind === 'write-record' ||
				operation.kind === 'tombstone-record'
		).length;
		const indexChanges = captured.operations.length - recordChanges;
		const nextSequence = host.getDiagnosticLayerSequence() + 1;
		host.setDiagnosticLayerSequence(nextSequence);
		const layer = Object.freeze({
			id,
			sequence: nextSequence,
			state: 'optimistic' as const,
			recordChanges,
			indexChanges,
			semanticChanges: recordChanges + semanticChanges.length
		});
		host.diagnosticLayers.set(id, layer);
		host.diagnosticEvent(
			Object.freeze({
				kind: 'layer',
				layer: id,
				action: 'created',
				recordChanges,
				indexChanges
			})
		);
		host.diagnosticEvent(
			Object.freeze({
				kind: 'receipt',
				command: id,
				state: 'optimistic',
				obligations: 0,
				observed: 0
			})
		);
		host.syncDiagnostics();
	}
}

export function markOptimisticLayerAcceptedOn(
	host: OptimisticHost,
	id: string,
	receipt?: DistributedCommandMetadata
): boolean {
	if (receipt !== undefined && receipt.commandId !== id) {
		throw new TypeError('optimistic layer id must equal the causal command id');
	}
	const accepted = host.engine.markOptimisticLayerAccepted(id);
	if (!accepted) return false;
	const diagnosticLayer = host.diagnosticLayers?.get(id);
	if (diagnosticLayer !== undefined) {
		host.diagnosticLayers!.set(
			id,
			Object.freeze({ ...diagnosticLayer, state: 'accepted' as const })
		);
		host.diagnosticEvent(
			Object.freeze({
				kind: 'layer',
				layer: id,
				action: 'accepted',
				recordChanges: diagnosticLayer.recordChanges,
				indexChanges: diagnosticLayer.indexChanges
			})
		);
	}
	if (receipt === undefined) {
		if (host.diagnostics.enabled) {
			host.diagnosticEvent(
				Object.freeze({
					kind: 'receipt',
					command: id,
					state: 'succeeded',
					obligations: 0,
					observed: 0
				})
			);
		}
		host.syncDiagnostics();
		return true;
	}
	const next = optimisticReceiptState(receipt);
	const current = host.optimisticReceipts.get(id);
	if (current !== undefined && !sameReceipt(current, next)) {
		throw new DistributedProtocolError(
			'DISTRIBUTED_PROTOCOL_INVALID',
			'extensions.distributed.command'
		);
	}
	host.optimisticReceipts.set(id, next);
	if (host.diagnostics.enabled) {
		const counts = diagnosticReceiptCounts(next);
		host.diagnosticEvent(
			Object.freeze({
				kind: 'receipt',
				command: id,
				state:
					counts.obligations === 0
						? ('succeeded' as const)
						: ('succeeded_pending_projection' as const),
				obligations: counts.obligations,
				observed: counts.observed
			})
		);
	}
	host.syncDiagnostics();
	return true;
}

export function confirmOptimisticLayerOn<T>(
	host: OptimisticHost,
	id: string,
	update: (writer: ReplicaBaseWriter) => T
): T {
	const result = host.engine.confirmOptimisticLayer(id, (writer) =>
		update(baseWriter(writer))
	);
	host.retireDiagnosticLayer(id, 'retired', 'projected');
	host.optimisticReceipts.delete(id);
	host.syncDiagnostics();
	return result;
}

export function rejectOptimisticLayerOn(
	host: OptimisticHost,
	id: string
): boolean {
	const rejected = host.engine.rejectOptimisticLayer(id);
	if (rejected) {
		host.retireDiagnosticLayer(id, 'rejected', 'rejected');
		host.optimisticReceipts.delete(id);
		host.syncDiagnostics();
	}
	return rejected;
}

export function planOptimisticReceipts(
	host: OptimisticHost,
	command: DistributedCommandMetadata | undefined,
	observations: readonly DistributedProjectionObservation[],
	satisfactionAdmissible: boolean
): {
	updates: Map<string, OptimisticReceiptState>;
	satisfied: string[];
} {
	const updates = new Map<string, OptimisticReceiptState>();
	if (
		command !== undefined &&
		host.engine.optimisticLayerState(command.commandId) !== undefined
	) {
		const proposed = optimisticReceiptState(command);
		const current = host.optimisticReceipts.get(command.commandId);
		if (current !== undefined && !sameReceipt(current, proposed)) {
			protocolInvalid('extensions.distributed.command');
		}
		updates.set(
			command.commandId,
			cloneOptimisticReceipt(current ?? proposed)
		);
	}
	for (const [id, receipt] of host.optimisticReceipts) {
		if (!updates.has(id)) {
			updates.set(id, cloneOptimisticReceipt(receipt));
		}
	}
	for (const receipt of updates.values()) {
		for (const observation of observations) {
			if (observation.causationId !== receipt.causationId) continue;
			const key = expectationKey(observation);
			if (receipt.expectations.has(key)) receipt.observed.add(key);
		}
	}
	const satisfied = satisfactionAdmissible
		? [...updates]
				.filter(
					([, receipt]) =>
						receipt.expectations.size > 0 &&
						[...receipt.expectations.keys()].every((key) =>
							receipt.observed.has(key)
						)
				)
				.map(([id]) => id)
		: [];
	return { updates, satisfied };
}

export function applyReceiptOnly(
	host: OptimisticHost,
	command: DistributedCommandMetadata | undefined
): void {
	if (command === undefined) return;
	const plan = planOptimisticReceipts(
		host,
		command,
		command.observations,
		true
	);
	for (const [id, receipt] of plan.updates) {
		host.optimisticReceipts.set(id, receipt);
		host.engine.markOptimisticLayerAccepted(id);
		const layer = host.diagnosticLayers?.get(id);
		if (layer !== undefined) {
			host.diagnosticLayers!.set(
				id,
				Object.freeze({ ...layer, state: 'accepted' as const })
			);
		}
		if (host.diagnostics.enabled) {
			const counts = diagnosticReceiptCounts(receipt);
			host.diagnosticEvent(
				Object.freeze({
					kind: 'receipt',
					command: id,
					state:
						counts.obligations === 0
							? ('succeeded' as const)
							: ('succeeded_pending_projection' as const),
					obligations: counts.obligations,
					observed: counts.observed
				})
			);
		}
	}
}
