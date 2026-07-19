/**
 * GENERATED — do not edit by hand.
 * Source: e2e_service::graphql_commands() → commands.manifest.json
 * Documents mirror `commands.operations.gql` (copy-paste for GraphiQL).
 * Regenerate: `make gen-commands` (from tests/e2e-ui)
 * Spec: distributed GitKB specs/query-layer/references/command-client-dx
 */
import type { GqlDocument } from '../gql/document.ts';
import type { GqlResult } from '../gql/types.ts';

/** Bound client from `useGraphql(() => data)` / `createGraphqlClient`. */
export type CommandClient = {
  request: <
    TResult = Record<string, unknown>,
    TVariables extends Record<string, unknown> = Record<string, unknown>
  >(
    document: GqlDocument,
    variables?: TVariables
  ) => Promise<GqlResult<TResult>>;
};

export type TodoCreateInput = {
  todo_id: string;
  title: string;
};

export type TodoCreatePayload = {
  todo_id: string;
  owner_id: string;
  title: string;
  status: string;
};

export type TodoCompleteInput = {
  todo_id: string;
};

export type TodoStatusPayload = {
  todo_id: string;
  status: string;
};

export type TodoArchiveInput = {
  todo_id: string;
};

export type TodoForceArchiveInput = {
  todo_id: string;
};

export type TodoForceArchivePayload = {
  todo_id: string;
  owner_id: string;
  status: string;
  archived_by: string;
};

export type TodoRenameInput = {
  todo_id: string;
  title: string;
};

export type TodoRenamePayload = {
  todo_id: string;
  title: string;
  status: string;
};

export type TodoReopenInput = {
  todo_id: string;
};

export type ChatPostInput = {
  message_id: string;
  body: string;
  room_id: string;
};

export type ChatPostPayload = {
  message_id: string;
  room_id: string;
  author_id: string;
  body: string;
  created_at: string;
};

export type BlobStartInput = {
  game_id: string;
};

export type BlobGamePayload = {
  game_id: string;
  owner_id: string;
  score: number;
  player_dead: boolean;
  current_level: number;
  current_level_completed: boolean;
  map_json: string;
  status: string;
};

export type BlobMoveInput = {
  game_id: string;
  direction: string;
};

export type BlobStartLevelInput = {
  game_id: string;
};

/** Field name → roles that may execute (engine ACL; client is not a boundary). */
export const COMMAND_ROLES = {
  "todos_create": ["user", "admin"] as const,
  "todos_complete": ["user", "admin"] as const,
  "todos_archive": ["user", "admin"] as const,
  "todos_force_archive": ["admin"] as const,
  "todos_rename": ["user", "admin"] as const,
  "todos_reopen": ["user", "admin"] as const,
  "chat_messages_post": ["user", "admin"] as const,
  "blob_games_start": ["user", "admin"] as const,
  "blob_games_move": ["user", "admin"] as const,
  "blob_games_start_level": ["user", "admin"] as const,
} as const;

/** GraphQL mutation documents — keep in sync with commands.operations.gql. */
export const COMMAND_DOCS = {
  "todos_create": `
mutation Command_todos_create($input: TodoCreateInput!) {
  todos_create(input: $input) {
    todo_id
    owner_id
    title
    status
  }
}
`,
  "todos_complete": `
mutation Command_todos_complete($input: TodoCompleteInput!) {
  todos_complete(input: $input) {
    todo_id
    status
  }
}
`,
  "todos_archive": `
mutation Command_todos_archive($input: TodoArchiveInput!) {
  todos_archive(input: $input) {
    todo_id
    status
  }
}
`,
  "todos_force_archive": `
mutation Command_todos_force_archive($input: TodoForceArchiveInput!) {
  todos_force_archive(input: $input) {
    todo_id
    owner_id
    status
    archived_by
  }
}
`,
  "todos_rename": `
mutation Command_todos_rename($input: TodoRenameInput!) {
  todos_rename(input: $input) {
    todo_id
    title
    status
  }
}
`,
  "todos_reopen": `
mutation Command_todos_reopen($input: TodoReopenInput!) {
  todos_reopen(input: $input) {
    todo_id
    status
  }
}
`,
  "chat_messages_post": `
mutation Command_chat_messages_post($input: ChatPostInput!) {
  chat_messages_post(input: $input) {
    message_id
    room_id
    author_id
    body
    created_at
  }
}
`,
  "blob_games_start": `
mutation Command_blob_games_start($input: BlobStartInput!) {
  blob_games_start(input: $input) {
    game_id
    owner_id
    score
    player_dead
    current_level
    current_level_completed
    map_json
    status
  }
}
`,
  "blob_games_move": `
mutation Command_blob_games_move($input: BlobMoveInput!) {
  blob_games_move(input: $input) {
    game_id
    owner_id
    score
    player_dead
    current_level
    current_level_completed
    map_json
    status
  }
}
`,
  "blob_games_start_level": `
mutation Command_blob_games_start_level($input: BlobStartLevelInput!) {
  blob_games_start_level(input: $input) {
    game_id
    owner_id
    score
    player_dead
    current_level
    current_level_completed
    map_json
    status
  }
}
`,
} as const;

/**
 * todo.create → GraphQL `todos_create`
 * roles: user, admin
 * Prefer `client.commands.todosCreate(…)` via `bindCommands` / `useGraphql`.
 */
export async function todosCreate(input: TodoCreateInput, client: CommandClient): Promise<GqlResult<TodoCreatePayload>> {
  const result = await client.request<{ todos_create?: TodoCreatePayload }>(COMMAND_DOCS["todos_create"], { input });
  return {
    data: result.data?.todos_create,
    errors: result.errors,
    status: result.status
  };
}

/**
 * todo.complete → GraphQL `todos_complete`
 * roles: user, admin
 * Prefer `client.commands.todosComplete(…)` via `bindCommands` / `useGraphql`.
 */
export async function todosComplete(input: TodoCompleteInput, client: CommandClient): Promise<GqlResult<TodoStatusPayload>> {
  const result = await client.request<{ todos_complete?: TodoStatusPayload }>(COMMAND_DOCS["todos_complete"], { input });
  return {
    data: result.data?.todos_complete,
    errors: result.errors,
    status: result.status
  };
}

/**
 * todo.archive → GraphQL `todos_archive`
 * roles: user, admin
 * Prefer `client.commands.todosArchive(…)` via `bindCommands` / `useGraphql`.
 */
export async function todosArchive(input: TodoArchiveInput, client: CommandClient): Promise<GqlResult<TodoStatusPayload>> {
  const result = await client.request<{ todos_archive?: TodoStatusPayload }>(COMMAND_DOCS["todos_archive"], { input });
  return {
    data: result.data?.todos_archive,
    errors: result.errors,
    status: result.status
  };
}

/**
 * todo.force_archive → GraphQL `todos_force_archive`
 * roles: admin
 * Prefer `client.commands.todosForceArchive(…)` via `bindCommands` / `useGraphql`.
 */
export async function todosForceArchive(input: TodoForceArchiveInput, client: CommandClient): Promise<GqlResult<TodoForceArchivePayload>> {
  const result = await client.request<{ todos_force_archive?: TodoForceArchivePayload }>(COMMAND_DOCS["todos_force_archive"], { input });
  return {
    data: result.data?.todos_force_archive,
    errors: result.errors,
    status: result.status
  };
}

/**
 * todo.rename → GraphQL `todos_rename`
 * roles: user, admin
 * Prefer `client.commands.todosRename(…)` via `bindCommands` / `useGraphql`.
 */
export async function todosRename(input: TodoRenameInput, client: CommandClient): Promise<GqlResult<TodoRenamePayload>> {
  const result = await client.request<{ todos_rename?: TodoRenamePayload }>(COMMAND_DOCS["todos_rename"], { input });
  return {
    data: result.data?.todos_rename,
    errors: result.errors,
    status: result.status
  };
}

/**
 * todo.reopen → GraphQL `todos_reopen`
 * roles: user, admin
 * Prefer `client.commands.todosReopen(…)` via `bindCommands` / `useGraphql`.
 */
export async function todosReopen(input: TodoReopenInput, client: CommandClient): Promise<GqlResult<TodoStatusPayload>> {
  const result = await client.request<{ todos_reopen?: TodoStatusPayload }>(COMMAND_DOCS["todos_reopen"], { input });
  return {
    data: result.data?.todos_reopen,
    errors: result.errors,
    status: result.status
  };
}

/**
 * chat.post → GraphQL `chat_messages_post`
 * roles: user, admin
 * Prefer `client.commands.chatMessagesPost(…)` via `bindCommands` / `useGraphql`.
 */
export async function chatMessagesPost(input: ChatPostInput, client: CommandClient): Promise<GqlResult<ChatPostPayload>> {
  const result = await client.request<{ chat_messages_post?: ChatPostPayload }>(COMMAND_DOCS["chat_messages_post"], { input });
  return {
    data: result.data?.chat_messages_post,
    errors: result.errors,
    status: result.status
  };
}

/**
 * blob.start → GraphQL `blob_games_start`
 * roles: user, admin
 * Prefer `client.commands.blobGamesStart(…)` via `bindCommands` / `useGraphql`.
 */
export async function blobGamesStart(input: BlobStartInput, client: CommandClient): Promise<GqlResult<BlobGamePayload>> {
  const result = await client.request<{ blob_games_start?: BlobGamePayload }>(COMMAND_DOCS["blob_games_start"], { input });
  return {
    data: result.data?.blob_games_start,
    errors: result.errors,
    status: result.status
  };
}

/**
 * blob.move → GraphQL `blob_games_move`
 * roles: user, admin
 * Prefer `client.commands.blobGamesMove(…)` via `bindCommands` / `useGraphql`.
 */
export async function blobGamesMove(input: BlobMoveInput, client: CommandClient): Promise<GqlResult<BlobGamePayload>> {
  const result = await client.request<{ blob_games_move?: BlobGamePayload }>(COMMAND_DOCS["blob_games_move"], { input });
  return {
    data: result.data?.blob_games_move,
    errors: result.errors,
    status: result.status
  };
}

/**
 * blob.start_level → GraphQL `blob_games_start_level`
 * roles: user, admin
 * Prefer `client.commands.blobGamesStartLevel(…)` via `bindCommands` / `useGraphql`.
 */
export async function blobGamesStartLevel(input: BlobStartLevelInput, client: CommandClient): Promise<GqlResult<BlobGamePayload>> {
  const result = await client.request<{ blob_games_start_level?: BlobGamePayload }>(COMMAND_DOCS["blob_games_start_level"], { input });
  return {
    data: result.data?.blob_games_start_level,
    errors: result.errors,
    status: result.status
  };
}

/** Commands pre-bound to a GraphQL client (URL + auth already configured). */
export type BoundCommands = {
  todosCreate: (input: TodoCreateInput) => Promise<GqlResult<TodoCreatePayload>>;
  todosComplete: (input: TodoCompleteInput) => Promise<GqlResult<TodoStatusPayload>>;
  todosArchive: (input: TodoArchiveInput) => Promise<GqlResult<TodoStatusPayload>>;
  todosForceArchive: (input: TodoForceArchiveInput) => Promise<GqlResult<TodoForceArchivePayload>>;
  todosRename: (input: TodoRenameInput) => Promise<GqlResult<TodoRenamePayload>>;
  todosReopen: (input: TodoReopenInput) => Promise<GqlResult<TodoStatusPayload>>;
  chatMessagesPost: (input: ChatPostInput) => Promise<GqlResult<ChatPostPayload>>;
  blobGamesStart: (input: BlobStartInput) => Promise<GqlResult<BlobGamePayload>>;
  blobGamesMove: (input: BlobMoveInput) => Promise<GqlResult<BlobGamePayload>>;
  blobGamesStartLevel: (input: BlobStartLevelInput) => Promise<GqlResult<BlobGamePayload>>;
};

/**
 * Register all command helpers on a client once:
 * `const gql = useGraphql(() => data); await gql.commands.todosCreate(input)`
 */
export function bindCommands(client: CommandClient): BoundCommands {
  return {
    todosCreate: (input) => todosCreate(input, client),
    todosComplete: (input) => todosComplete(input, client),
    todosArchive: (input) => todosArchive(input, client),
    todosForceArchive: (input) => todosForceArchive(input, client),
    todosRename: (input) => todosRename(input, client),
    todosReopen: (input) => todosReopen(input, client),
    chatMessagesPost: (input) => chatMessagesPost(input, client),
    blobGamesStart: (input) => blobGamesStart(input, client),
    blobGamesMove: (input) => blobGamesMove(input, client),
    blobGamesStartLevel: (input) => blobGamesStartLevel(input, client),
  };
}
