/**
 * Bind generated commands through the command result pipeline when a QueryCache
 * is present. Call sites:
 *
 *   await gql.commands.todosCreate(input, {
 *     optimistic: { targets: [...], row },
 *     result: { kind: 'fact' },
 *     reconcile: { kind: 'refetch', document: todosDoc },
 *     onError: ({ errors }) => [effect.alert(errors[0]?.message ?? 'Failed')],
 *   });
 *
 * Without a second arg, defaults from `commandPolicies` apply when provided.
 */
import {
	COMMAND_DOCS,
	bindCommands,
	type BoundCommands,
	type CommandClient,
	type TodoCreateInput,
	type TodoCreatePayload,
	type TodoCompleteInput,
	type TodoStatusPayload,
	type TodoArchiveInput,
	type TodoForceArchiveInput,
	type TodoForceArchivePayload,
	type TodoRenameInput,
	type TodoRenamePayload,
	type TodoReopenInput,
	type ChatPostInput,
	type ChatPostPayload,
	type BlobStartInput,
	type BlobMoveInput,
	type BlobStartLevelInput,
	type BlobGamePayload
} from '../api/commands.generated.ts';
import type { GqlResult } from './types.ts';
import type { QueryCache } from './cache/query-cache.ts';
import {
	runCommandPipeline,
	type CommandPipelineOptions,
	type CommandPolicy
} from './cache/index.ts';

export type CommandCallOptions = CommandPipelineOptions;

/** Default policies keyed by bound command function name. */
export type CommandPolicyMap = Partial<Record<keyof BoundCommands, CommandPolicy>>;

export type PipelinedBoundCommands = {
	todosCreate: (
		input: TodoCreateInput,
		opts?: CommandCallOptions
	) => Promise<GqlResult<TodoCreatePayload>>;
	todosComplete: (
		input: TodoCompleteInput,
		opts?: CommandCallOptions
	) => Promise<GqlResult<TodoStatusPayload>>;
	todosArchive: (
		input: TodoArchiveInput,
		opts?: CommandCallOptions
	) => Promise<GqlResult<TodoStatusPayload>>;
	todosForceArchive: (
		input: TodoForceArchiveInput,
		opts?: CommandCallOptions
	) => Promise<GqlResult<TodoForceArchivePayload>>;
	todosRename: (
		input: TodoRenameInput,
		opts?: CommandCallOptions
	) => Promise<GqlResult<TodoRenamePayload>>;
	todosReopen: (
		input: TodoReopenInput,
		opts?: CommandCallOptions
	) => Promise<GqlResult<TodoStatusPayload>>;
	chatMessagesPost: (
		input: ChatPostInput,
		opts?: CommandCallOptions
	) => Promise<GqlResult<ChatPostPayload>>;
	blobGamesStart: (
		input: BlobStartInput,
		opts?: CommandCallOptions
	) => Promise<GqlResult<BlobGamePayload>>;
	blobGamesMove: (
		input: BlobMoveInput,
		opts?: CommandCallOptions
	) => Promise<GqlResult<BlobGamePayload>>;
	blobGamesStartLevel: (
		input: BlobStartLevelInput,
		opts?: CommandCallOptions
	) => Promise<GqlResult<BlobGamePayload>>;
};

type FieldName = keyof typeof COMMAND_DOCS;

const FN_TO_FIELD: Record<keyof BoundCommands, FieldName> = {
	todosCreate: 'todos_create',
	todosComplete: 'todos_complete',
	todosArchive: 'todos_archive',
	todosForceArchive: 'todos_force_archive',
	todosRename: 'todos_rename',
	todosReopen: 'todos_reopen',
	chatMessagesPost: 'chat_messages_post',
	blobGamesStart: 'blob_games_start',
	blobGamesMove: 'blob_games_move',
	blobGamesStartLevel: 'blob_games_start_level'
};

function isBrowser(): boolean {
	return typeof window !== 'undefined';
}

function unwrapField<T>(
	data: Record<string, unknown> | null | undefined,
	field: string
): T | undefined {
	if (!data || typeof data !== 'object') return undefined;
	return data[field] as T | undefined;
}

/**
 * Bind commands. With `cache`, each call runs optimistic → network → result →
 * effects → reconcile. Without cache (SSR), falls back to plain `bindCommands`.
 */
export function bindCommandsPipeline(
	client: CommandClient,
	options: {
		cache?: QueryCache;
		policies?: CommandPolicyMap;
		/** Collect UI effects (toast/alert) from onSuccess/onError. */
		runEffects?: (effects: import('./cache/ops.ts').Effect[]) => void;
	} = {}
): PipelinedBoundCommands {
	const plain = bindCommands(client);
	const cache = options.cache;
	const policies = options.policies ?? {};

	if (!cache) {
		// Still accept optional second arg for API compatibility; ignore it.
		return {
			todosCreate: (input, _opts?) => plain.todosCreate(input),
			todosComplete: (input, _opts?) => plain.todosComplete(input),
			todosArchive: (input, _opts?) => plain.todosArchive(input),
			todosForceArchive: (input, _opts?) => plain.todosForceArchive(input),
			todosRename: (input, _opts?) => plain.todosRename(input),
			todosReopen: (input, _opts?) => plain.todosReopen(input),
			chatMessagesPost: (input, _opts?) => plain.chatMessagesPost(input),
			blobGamesStart: (input, _opts?) => plain.blobGamesStart(input),
			blobGamesMove: (input, _opts?) => plain.blobGamesMove(input),
			blobGamesStartLevel: (input, _opts?) => plain.blobGamesStartLevel(input)
		};
	}

	const wrap =
		<I extends Record<string, unknown>, O>(fnName: keyof BoundCommands) =>
		async (input: I, callOpts: CommandCallOptions = {}): Promise<GqlResult<O>> => {
			const field = FN_TO_FIELD[fnName];
			const document = COMMAND_DOCS[field];
			const policy = policies[fnName];
			// Always use the pipeline when a cache is bound. Optimistic/cache mutations
			// run when `browser` is true (real window, or explicit opt-in for unit tests).
			const browser = callOpts.browser ?? isBrowser();

			const result = await runCommandPipeline(
				{
					cache,
					request: async (doc, variables) => {
						const r = await client.request(doc, variables);
						return { data: r.data as Record<string, unknown> | null, errors: r.errors };
					},
					refetch: async (doc, variables) => {
						const r = await client.request(doc, variables);
						return { data: r.data as Record<string, unknown> | null, errors: r.errors };
					},
					runEffects: options.runEffects
				},
				document,
				input,
				{
					...callOpts,
					policy: callOpts.policy ?? policy,
					browser
				}
			);

			const errors = result.errors?.map((e) => ({
				message: e.message ?? 'GraphQL error',
				extensions: e.extensions
			}));
			return {
				data: unwrapField<O>(
					result.data as Record<string, unknown> | null | undefined,
					field
				),
				errors,
				status: 200
			};
		};

	return {
		todosCreate: wrap('todosCreate'),
		todosComplete: wrap('todosComplete'),
		todosArchive: wrap('todosArchive'),
		todosForceArchive: wrap('todosForceArchive'),
		todosRename: wrap('todosRename'),
		todosReopen: wrap('todosReopen'),
		chatMessagesPost: wrap('chatMessagesPost'),
		blobGamesStart: wrap('blobGamesStart'),
		blobGamesMove: wrap('blobGamesMove'),
		blobGamesStartLevel: wrap('blobGamesStartLevel')
	};
}
