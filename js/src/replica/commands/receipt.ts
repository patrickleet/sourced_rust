import type { DistributedCommandMetadata } from '../../protocol.js';
import type {
	ReplicaPreparedCommand,
	ReplicaReceiptVerification
} from './types.js';
import {
	receiptMismatch
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
	if (receipt.projectionDisposition === 'revalidate') {
		if (
			prepared.projection === undefined &&
			!prepared.revalidation.required
		) {
			receiptMismatch('receipt.projectionDisposition');
		}
		return Object.freeze({
			kind:
				receipt.state === 'succeeded_pending_projection'
					? 'deferred'
					: 'matched',
			revalidate: true
		});
	}
	if (receipt.state === 'in_progress' && receipt.expects.length === 0) {
		return Object.freeze({
			kind: 'deferred',
			revalidate:
				prepared.revalidation.required ||
				prepared.projection?.revalidate === true
		});
	}
	if (prepared.projection === undefined) {
		if (receipt.projection !== undefined || receipt.expects.length !== 0) {
			receiptMismatch('receipt.expects');
		}
	} else if (receipt.projection === undefined) {
		receiptMismatch('receipt.expects');
	}
	return Object.freeze({
		kind: receipt.state === 'in_progress' ? 'deferred' : 'matched',
		revalidate:
			prepared.revalidation.required ||
			prepared.projection?.revalidate === true
	});
}
