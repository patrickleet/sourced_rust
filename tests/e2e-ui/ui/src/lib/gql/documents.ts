/**
 * Shared GraphQL documents.
 * Todos ops live in `routes/todos/todos.resource.ts` (co-located defineResource).
 * Re-export here so any remaining imports keep the same string identity as `todos.query`.
 */
import { todos } from '../../routes/todos/todos.resource';

export const TODOS_QUERY = todos.query;
export const TODOS_CREATE = todos.mutations.create;
export const TODOS_COMPLETE = todos.mutations.complete;
export const TODOS_ARCHIVE = todos.mutations.archive;

export function chatMessagesQuery(room: string): string {
	return `{
  chat_messages(where: { room_id: { _eq: "${room}" } }) {
    message_id
    room_id
    author_id
    body
    created_at
  }
}`;
}

export function chatMessagesSubscription(room: string): string {
	return `subscription {
  chat_messages(where: { room_id: { _eq: "${room}" } }) {
    message_id
    room_id
    author_id
    body
    created_at
  }
}`;
}

export const CHAT_POST = `mutation ChatPost($message_id: String!, $body: String!, $room_id: String!) {
  chat_messages_post(input: {
    message_id: $message_id
    body: $body
    room_id: $room_id
  }) {
    message_id
    room_id
    author_id
    body
    created_at
  }
}`;
