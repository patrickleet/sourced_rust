import type { ReplicaCommandContractErrorCode } from './types.js';

export function commandErrorMessage(
	code: ReplicaCommandContractErrorCode,
	path: string
): string {
	switch (code) {
		case 'REPLICA_COMMAND_ARTIFACT_INVALID':
			return `Invalid generated replica command artifact at ${path}`;
		case 'REPLICA_COMMAND_INPUT_INVALID':
			return `Invalid replica command input at ${path}`;
		case 'REPLICA_COMMAND_RECEIPT_MISMATCH':
			return `Replica command receipt does not match ${path}`;
		case 'REPLICA_COMMAND_TRUSTED_PRESET_MISMATCH':
			return `Authoritative trusted preset inventory does not match ${path}`;
	}
}

export class ReplicaCommandContractError extends Error {
	readonly code: ReplicaCommandContractErrorCode;
	readonly path: string;

	constructor(code: ReplicaCommandContractErrorCode, path: string) {
		super(commandErrorMessage(code, path));
		this.name = 'ReplicaCommandContractError';
		this.code = code;
		this.path = path;
	}
}
