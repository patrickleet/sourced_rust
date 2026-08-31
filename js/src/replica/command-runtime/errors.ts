import type { DistributedCommandState } from '../../protocol.js';
import type {
	ReplicaCommandRecoveryReceipt,
	ReplicaCommandRuntimeErrorCode
} from './types.js';

export function commandRuntimeErrorMessage(
	code: ReplicaCommandRuntimeErrorCode
): string {
	switch (code) {
		case 'REPLICA_COMMAND_ABORTED':
			return 'Caller aborted command dispatch or projection visibility wait';
		case 'REPLICA_COMMAND_AUTHORITY_UNAVAILABLE':
			return 'Command dispatch requires a current authoritative replica scope';
		case 'REPLICA_COMMAND_DISPOSED':
			return 'Command runtime is disposed';
		case 'REPLICA_COMMAND_OUTCOME_PENDING':
			return 'Command outcome remains pending';
		case 'REPLICA_COMMAND_PROJECTION_FAILED':
			return 'Command projection failed';
		case 'REPLICA_COMMAND_PROTOCOL_INVALID':
			return 'Command response violated the generated protocol contract';
		case 'REPLICA_COMMAND_REJECTED':
			return 'Command was rejected';
		case 'REPLICA_COMMAND_RELOADING':
			return 'Command dispatch is paused during a coherent application reload';
		case 'REPLICA_COMMAND_SCOPE_INVALIDATED':
			return 'Command authorization scope changed';
		case 'REPLICA_COMMAND_STATUS_UNAVAILABLE':
			return 'Generated command status transport is unavailable';
		case 'REPLICA_COMMAND_TRANSPORT_AMBIGUOUS':
			return 'Command transport outcome is ambiguous';
		}
}

/** Redacted typed failure from the integrated generated-command lifecycle. */
export class ReplicaCommandRuntimeError extends Error {
	readonly code: ReplicaCommandRuntimeErrorCode;
	readonly commandId?: string;
	readonly state?: DistributedCommandState;
	readonly recovery?: ReplicaCommandRecoveryReceipt;

	constructor(
		code: ReplicaCommandRuntimeErrorCode,
		options: Readonly<{
			commandId?: string;
			state?: DistributedCommandState;
			cause?: unknown;
			recovery?: ReplicaCommandRecoveryReceipt;
		}> = {}
	) {
		super(commandRuntimeErrorMessage(code), {
			...(options.cause === undefined ? {} : { cause: options.cause })
		});
		this.name = 'ReplicaCommandRuntimeError';
		this.code = code;
		this.commandId = options.commandId;
		this.state = options.state;
		this.recovery = options.recovery;
	}
}
