/**
 * Default command result/reconcile policies for e2e-ui.
 * Async projectors → fact + reconcile none (no immediate refetch).
 * Call sites override with optimistic / onError only.
 */
import type { CommandPolicyMap } from './bind-commands-pipeline.ts';

/** Shared defaults for todos + chat (async projector topology). */
export const e2eCommandPolicies: CommandPolicyMap = {
	todosCreate: {
		result: { kind: 'fact' },
		reconcile: { kind: 'none' }
	},
	todosComplete: {
		result: { kind: 'fact' },
		reconcile: { kind: 'none' }
	},
	todosArchive: {
		result: { kind: 'fact' },
		reconcile: { kind: 'none' }
	},
	todosForceArchive: {
		result: { kind: 'fact' },
		reconcile: { kind: 'none' }
	},
	todosRename: {
		result: { kind: 'fact' },
		reconcile: { kind: 'none' }
	},
	todosReopen: {
		result: { kind: 'fact' },
		reconcile: { kind: 'none' }
	},
	chatMessagesPost: {
		result: { kind: 'fact' },
		reconcile: { kind: 'subscription' }
	}
};
