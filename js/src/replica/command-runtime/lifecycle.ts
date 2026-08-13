import { replicaCommandProjectedLifecycle } from './symbols.js';
import type {
	ReplicaCommandProjectedOutcome,
	ReplicaCommandReceipt,
	ReplicaCommandReceiptWithLifecycle
} from './types.js';

/**
 * Package-private causal lifecycle used by framework adapters.
 *
 * A caller-scoped `receipt.projected` can reject when its AbortSignal fires
 * after acceptance. The underlying command remains globally pending until
 * canonical projection evidence settles this independent promise.
 *
 * @internal
 */
export function replicaCommandProjectedLifecycleOf(
	receipt: ReplicaCommandReceipt<unknown>
): Promise<ReplicaCommandProjectedOutcome<unknown>> | undefined {
	return (receipt as ReplicaCommandReceiptWithLifecycle)[
		replicaCommandProjectedLifecycle
	];
}
