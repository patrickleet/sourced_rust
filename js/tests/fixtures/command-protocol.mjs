export const COMMAND_CONSISTENCY = Object.freeze({
	SUCCEEDED: 'succeeded',
	EVENTUAL: 'eventual',
	ATOMIC: 'atomic'
});

export const COMMAND_STATE = Object.freeze({
	SUCCEEDED: 'succeeded',
	PENDING_PROJECTION: 'succeeded_pending_projection',
	IN_PROGRESS: 'in_progress',
	ATOMIC: 'atomic'
});

/**
 * Canonical valid command metadata for behavior tests. Malformed/fail-closed
 * tests should continue to spell out their deliberately invalid payloads.
 */
export function commandReceipt(overrides = {}) {
	return {
		commandId: 'opaque-command-id',
		causationId: 'opaque-causation-id',
		state: COMMAND_STATE.PENDING_PROJECTION,
		consistency: COMMAND_CONSISTENCY.EVENTUAL,
		expects: [],
		observations: [],
		records: [],
		...overrides
	};
}
