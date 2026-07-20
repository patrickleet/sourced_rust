/**
 * GENERATED — do not edit by hand.
 * Source: e2e_service::graphql_commands() → client_reconcile → commands.manifest.json
 * Regenerate: `make gen-commands` (from tests/e2e-ui)
 * Spec: distributed GitKB tasks/graphql-qs-command-return-4 (D3)
 */
import type { CommandPolicyMap } from '../gql/bind-commands-pipeline.ts';

/**
 * Default result/reconcile policies from the Rust command registry.
 * Call-site options on `gql.commands.*(input, opts)` still win.
 */
export const e2eCommandPolicies: CommandPolicyMap = {
	todosCreate: {
		result: { kind: "fact" },
		reconcile: { kind: "none" }
	},
	todosComplete: {
		result: { kind: "fact" },
		reconcile: { kind: "none" }
	},
	todosArchive: {
		result: { kind: "fact" },
		reconcile: { kind: "none" }
	},
	todosForceArchive: {
		result: { kind: "fact" },
		reconcile: { kind: "none" }
	},
	todosRename: {
		result: { kind: "fact" },
		reconcile: { kind: "none" }
	},
	todosReopen: {
		result: { kind: "fact" },
		reconcile: { kind: "none" }
	},
	chatMessagesPost: {
		result: { kind: "fact" },
		reconcile: { kind: "subscription" }
	},
	blobGamesStart: {
		result: { kind: "projection" },
		reconcile: { kind: "none" }
	},
	blobGamesMove: {
		result: { kind: "projection" },
		reconcile: { kind: "none" }
	},
	blobGamesStartLevel: {
		result: { kind: "projection" },
		reconcile: { kind: "none" }
	},
};
