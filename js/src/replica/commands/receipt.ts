import type { DistributedCommandMetadata } from '../../protocol.js';
import type {
	ReplicaPreparedCommand,
	ReplicaReceiptVerification
} from './types.js';
import {
	receiptMismatch,
	sameProjectionMultiset
} from './util.js';

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

