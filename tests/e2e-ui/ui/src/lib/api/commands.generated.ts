/**
 * GENERATED — do not edit by hand.
 * Source: e2e_service::graphql_commands() → commands.manifest.json
 * Regenerate: `make gen-commands` (from tests/e2e-ui)
 * Spec: distributed GitKB specs/query-layer/references/command-client-dx
 */
import { requestGraphql } from '../gql/request.ts';
import type { GqlAuth, GqlResult } from '../gql/types.ts';

export type CommandRequestOpts = {
  /** Absolute or same-origin GraphQL URL, e.g. `/graphql` */
  url: string;
  auth?: GqlAuth;
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

/** Field name → roles that may execute (engine ACL; client is not a boundary). */
export const COMMAND_ROLES = {
  "todos_create": ["user", "admin"] as const,
  "todos_complete": ["user", "admin"] as const,
  "todos_archive": ["user", "admin"] as const,
  "todos_force_archive": ["admin"] as const,
  "todos_rename": ["user", "admin"] as const,
  "todos_reopen": ["user", "admin"] as const,
  "chat_messages_post": ["user", "admin"] as const,
} as const;

/**
 * todo.create → GraphQL `todos_create`
 * roles: user, admin
 */
export async function todosCreate(input: TodoCreateInput, opts: CommandRequestOpts): Promise<GqlResult<TodoCreatePayload>> {
  const document = `
mutation Command_todos_create($input: TodoCreateInput!) {
  todos_create(input: $input) {
    todo_id
    owner_id
    title
    status
  }
}
`;
  const result = await requestGraphql<{ todos_create?: TodoCreatePayload }>(opts.url, document, opts.auth ?? {}, { input });
  return {
    data: result.data?.todos_create,
    errors: result.errors,
    status: result.status
  };
}

/**
 * todo.complete → GraphQL `todos_complete`
 * roles: user, admin
 */
export async function todosComplete(input: TodoCompleteInput, opts: CommandRequestOpts): Promise<GqlResult<TodoStatusPayload>> {
  const document = `
mutation Command_todos_complete($input: TodoCompleteInput!) {
  todos_complete(input: $input) {
    todo_id
    status
  }
}
`;
  const result = await requestGraphql<{ todos_complete?: TodoStatusPayload }>(opts.url, document, opts.auth ?? {}, { input });
  return {
    data: result.data?.todos_complete,
    errors: result.errors,
    status: result.status
  };
}

/**
 * todo.archive → GraphQL `todos_archive`
 * roles: user, admin
 */
export async function todosArchive(input: TodoArchiveInput, opts: CommandRequestOpts): Promise<GqlResult<TodoStatusPayload>> {
  const document = `
mutation Command_todos_archive($input: TodoArchiveInput!) {
  todos_archive(input: $input) {
    todo_id
    status
  }
}
`;
  const result = await requestGraphql<{ todos_archive?: TodoStatusPayload }>(opts.url, document, opts.auth ?? {}, { input });
  return {
    data: result.data?.todos_archive,
    errors: result.errors,
    status: result.status
  };
}

/**
 * todo.force_archive → GraphQL `todos_force_archive`
 * roles: admin
 */
export async function todosForceArchive(input: TodoForceArchiveInput, opts: CommandRequestOpts): Promise<GqlResult<TodoForceArchivePayload>> {
  const document = `
mutation Command_todos_force_archive($input: TodoForceArchiveInput!) {
  todos_force_archive(input: $input) {
    todo_id
    owner_id
    status
    archived_by
  }
}
`;
  const result = await requestGraphql<{ todos_force_archive?: TodoForceArchivePayload }>(opts.url, document, opts.auth ?? {}, { input });
  return {
    data: result.data?.todos_force_archive,
    errors: result.errors,
    status: result.status
  };
}

/**
 * todo.rename → GraphQL `todos_rename`
 * roles: user, admin
 */
export async function todosRename(input: TodoRenameInput, opts: CommandRequestOpts): Promise<GqlResult<TodoRenamePayload>> {
  const document = `
mutation Command_todos_rename($input: TodoRenameInput!) {
  todos_rename(input: $input) {
    todo_id
    title
    status
  }
}
`;
  const result = await requestGraphql<{ todos_rename?: TodoRenamePayload }>(opts.url, document, opts.auth ?? {}, { input });
  return {
    data: result.data?.todos_rename,
    errors: result.errors,
    status: result.status
  };
}

/**
 * todo.reopen → GraphQL `todos_reopen`
 * roles: user, admin
 */
export async function todosReopen(input: TodoReopenInput, opts: CommandRequestOpts): Promise<GqlResult<TodoStatusPayload>> {
  const document = `
mutation Command_todos_reopen($input: TodoReopenInput!) {
  todos_reopen(input: $input) {
    todo_id
    status
  }
}
`;
  const result = await requestGraphql<{ todos_reopen?: TodoStatusPayload }>(opts.url, document, opts.auth ?? {}, { input });
  return {
    data: result.data?.todos_reopen,
    errors: result.errors,
    status: result.status
  };
}

/**
 * chat.post → GraphQL `chat_messages_post`
 * roles: user, admin
 */
export async function chatMessagesPost(input: ChatPostInput, opts: CommandRequestOpts): Promise<GqlResult<ChatPostPayload>> {
  const document = `
mutation Command_chat_messages_post($input: ChatPostInput!) {
  chat_messages_post(input: $input) {
    message_id
    room_id
    author_id
    body
    created_at
  }
}
`;
  const result = await requestGraphql<{ chat_messages_post?: ChatPostPayload }>(opts.url, document, opts.auth ?? {}, { input });
  return {
    data: result.data?.chat_messages_post,
    errors: result.errors,
    status: result.status
  };
}
