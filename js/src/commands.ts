import { documentToString, type GqlDocument } from './document.js';
import type { GqlResult, GraphqlVariables } from './types.js';
import type { QueryCache } from './cache/query-cache.js';
import type { Effect } from './cache/ops.js';
import {
  runCommandPipeline,
  type CommandPipelineOptions,
  type CommandPolicy
} from './cache/pipeline.js';

/** The request surface needed by generated command clients. */
export type CommandClient = {
  request: <
    TResult = Record<string, unknown>,
    TVariables extends GraphqlVariables = GraphqlVariables
  >(
    document: GqlDocument<TResult, TVariables>,
    variables?: TVariables
  ) => Promise<GqlResult<TResult>>;
  /** Used automatically by bound commands unless a cache is supplied explicitly. */
  cache?: QueryCache;
};

declare const commandTypes: unique symbol;

/**
 * A command's runtime metadata and compile-time input/output contract.
 *
 * `hasInput` is explicit so zero-input commands never send an `{ input: ... }`
 * variable by accident.
 */
export type CommandDefinition<
  TInput extends object | void = object,
  TOutput = unknown
> = Readonly<{
  field: string;
  document: GqlDocument;
  hasInput: [TInput] extends [void] ? false : true;
  roles?: readonly string[];
  /** Type-only marker; `defineCommand` does not add it at runtime. */
  [commandTypes]?: {
    input: TInput;
    output: TOutput;
  };
}>;

export type AnyCommandDefinition =
  | CommandDefinition<object, unknown>
  | CommandDefinition<void, unknown>;

export type CommandDefinitionMap = Readonly<Record<string, AnyCommandDefinition>>;

export type CommandInput<TCommand> = TCommand extends CommandDefinition<
  infer TInput,
  unknown
>
  ? TInput
  : never;

export type CommandOutput<TCommand> = TCommand extends CommandDefinition<
  infer _TInput,
  infer TOutput
>
  ? TOutput
  : never;

/** Define one command while retaining its input/output types. */
export function defineCommand<TInput extends object | void, TOutput>(
  definition: CommandDefinition<TInput, TOutput>
): CommandDefinition<TInput, TOutput> {
  if (!definition.field.trim()) {
    throw new Error('command field must not be empty');
  }
  return Object.freeze({ ...definition });
}

/** Define an app-owned command collection while retaining every command key. */
export function defineCommands<const TCommands extends CommandDefinitionMap>(
  commands: TCommands
): TCommands {
  return Object.freeze({ ...commands }) as TCommands;
}

export type ExecuteCommandArgs<TCommand> = [CommandInput<TCommand>] extends [void]
  ? []
  : [input: CommandInput<TCommand>];

/** Execute a command directly, without client-cache policy handling. */
export async function executeCommand<TCommand extends AnyCommandDefinition>(
  client: CommandClient,
  command: TCommand,
  ...args: ExecuteCommandArgs<TCommand>
): Promise<GqlResult<CommandOutput<TCommand>>> {
  return executeCommandInternal(
    client,
    command,
    command.hasInput ? (args[0] as object) : undefined
  );
}

/** Per-call cache/pipeline overrides accepted by every bound command. */
export type CommandCallOptions = CommandPipelineOptions;

export type BoundCommand<TCommand> = [CommandInput<TCommand>] extends [void]
  ? (options?: CommandCallOptions) => Promise<GqlResult<CommandOutput<TCommand>>>
  : (
      input: CommandInput<TCommand>,
      options?: CommandCallOptions
    ) => Promise<GqlResult<CommandOutput<TCommand>>>;

export type BoundCommandSet<TCommands extends CommandDefinitionMap> = {
  [TName in keyof TCommands]: BoundCommand<TCommands[TName]>;
};

/** Default policies keyed by the app-owned command function name. */
export type CommandPolicyMap<TCommands extends CommandDefinitionMap> = Partial<{
  [TName in keyof TCommands]: CommandPolicy;
}>;

export type BindCommandsOptions<TCommands extends CommandDefinitionMap> = {
  /** Defaults to `client.cache` when the client exposes one. */
  cache?: QueryCache;
  policies?: CommandPolicyMap<TCommands>;
  /** Collect UI effects without coupling the command runtime to a framework. */
  runEffects?: (effects: Effect[]) => void;
};

/**
 * Bind a typed command collection to a client.
 *
 * When a cache is present, calls run through the command pipeline. Without a
 * cache, the same call shape executes directly and safely ignores call policy
 * options. This lets generated clients expose one stable API in browsers and
 * during SSR.
 */
export function bindCommands<TCommands extends CommandDefinitionMap>(
  client: CommandClient,
  commands: TCommands,
  options: BindCommandsOptions<TCommands> = {}
): BoundCommandSet<TCommands> {
  return bindCommandsPipeline(client, commands, options);
}

/** Bind commands with optional optimistic/cache policy behavior. */
export function bindCommandsPipeline<TCommands extends CommandDefinitionMap>(
  client: CommandClient,
  commands: TCommands,
  options: BindCommandsOptions<TCommands> = {}
): BoundCommandSet<TCommands> {
  const cache = options.cache ?? client.cache;
  const bound: Partial<Record<keyof TCommands, unknown>> = {};

  for (const name of Object.keys(commands) as Array<keyof TCommands>) {
    const command = commands[name]!;
    const policy = options.policies?.[name];

    if (command.hasInput) {
      bound[name] = (
        input: CommandInput<typeof command>,
        callOptions: CommandCallOptions = {}
      ) =>
        cache
          ? executePipelinedCommand(
              client,
              command,
              input as object,
              cache,
              policy,
              options.runEffects,
              callOptions
            )
          : executeCommandInternal(client, command, input as object);
    } else {
      bound[name] = (callOptions: CommandCallOptions = {}) =>
        cache
          ? executePipelinedCommand(
              client,
              command,
              undefined,
              cache,
              policy,
              options.runEffects,
              callOptions
            )
          : executeCommandInternal(client, command, undefined);
    }
  }

  return bound as BoundCommandSet<TCommands>;
}

async function executePipelinedCommand<
  TCommand extends AnyCommandDefinition
>(
  client: CommandClient,
  command: TCommand,
  input: object | undefined,
  cache: QueryCache,
  policy: CommandPolicy | undefined,
  runEffects: ((effects: Effect[]) => void) | undefined,
  callOptions: CommandCallOptions
): Promise<GqlResult<CommandOutput<TCommand>>> {
  let commandStatus = 0;
  const document = documentToString(command.document);
  const result = await runCommandPipeline<Record<string, CommandOutput<TCommand>>>(
    {
      cache,
      request: async (requestDocument, variables) => {
        const response = await client.request<Record<string, CommandOutput<TCommand>>>(
          requestDocument,
          variables
        );
        commandStatus = response.status;
        return response;
      },
      refetch: async (requestDocument, variables) => {
        const response = await client.request<Record<string, unknown>>(
          requestDocument,
          variables
        );
        return response;
      },
      runEffects
    },
    document,
    input as Record<string, unknown> | undefined,
    {
      ...callOptions,
      policy: callOptions.policy ?? policy,
      browser: callOptions.browser ?? hasBrowserWindow()
    }
  );

  // Pipeline errors may carry extensions. Keep them at runtime while ensuring
  // the shared result contract always has a useful message.
  const errors = result.errors?.map((error) => ({
    ...error,
    message: error.message ?? 'GraphQL error'
  }));

  return {
    data: unwrapField<CommandOutput<TCommand>>(result.data, command.field),
    errors,
    // A thrown transport request has no HTTP response; the pipeline returns 0.
    status: result.status ?? commandStatus
  };
}

async function executeCommandInternal<TOutput>(
  client: CommandClient,
  command: AnyCommandDefinition,
  input: object | undefined
): Promise<GqlResult<TOutput>> {
  const variables = command.hasInput
    ? { input: input as Record<string, unknown> }
    : undefined;
  const result = await client.request<Record<string, TOutput>>(
    command.document as GqlDocument<Record<string, TOutput>, GraphqlVariables>,
    variables
  );

  return {
    data: unwrapField<TOutput>(result.data, command.field),
    errors: result.errors,
    status: result.status
  };
}

function unwrapField<TOutput>(data: unknown, field: string): TOutput | undefined {
  if (!data || typeof data !== 'object' || Array.isArray(data)) return undefined;
  return (data as Record<string, unknown>)[field] as TOutput | undefined;
}

function hasBrowserWindow(): boolean {
  return typeof (globalThis as { window?: unknown }).window !== 'undefined';
}
