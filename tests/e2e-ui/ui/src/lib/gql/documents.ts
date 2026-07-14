/**
 * Shared GraphQL documents — re-exports from co-located resources so identity
 * matches `todos.query` / `chat.query` (and chat room helpers).
 */
import { todos } from '../../routes/todos/todos.resource';
import { chat, chatResource, LOBBY_ROOM } from '../../routes/chat/chat.resource';

export const TODOS_QUERY = todos.query;
export const TODOS_CREATE = todos.mutations.create;
export const TODOS_COMPLETE = todos.mutations.complete;
export const TODOS_ARCHIVE = todos.mutations.archive;

export const CHAT_POST = chat.mutations.post;

/** Same document family as `chatResource(room).query`. */
export function chatMessagesQuery(room: string): string {
	return chatResource(room).query;
}

/** Same document family as `chatResource(room).subscription`. */
export function chatMessagesSubscription(room: string): string {
	return chatResource(room).subscription ?? chatResource(room).query;
}

export { LOBBY_ROOM };
