/**
 * Shared document accessors — re-export wire strings from co-located resources
 * so identity matches generated TypedDocumentNode from `*.gql`.
 */
import { documentToString } from './document.ts';
import { todos } from '../../routes/todos/todos.resource';
import { chat, LOBBY_ROOM } from '../../routes/chat/chat.resource';

export const TODOS_QUERY = documentToString(todos.query);
export const TODOS_CREATE = documentToString(todos.mutations.create);
export const TODOS_COMPLETE = documentToString(todos.mutations.complete);
export const TODOS_ARCHIVE = documentToString(todos.mutations.archive);

export const CHAT_POST = documentToString(chat.mutations.post);

/** Lobby query string (same body as `chat.query` / ChatMessagesDocument). */
export function chatMessagesQuery(room: string): string {
	if (room !== LOBBY_ROOM) {
		throw new Error(
			`chatMessagesQuery: only room "${LOBBY_ROOM}" is codegen-backed; got "${room}"`
		);
	}
	return documentToString(chat.query);
}

/** Lobby subscription string (same body as `chat.subscription`). */
export function chatMessagesSubscription(room: string): string {
	if (room !== LOBBY_ROOM) {
		throw new Error(
			`chatMessagesSubscription: only room "${LOBBY_ROOM}" is codegen-backed; got "${room}"`
		);
	}
	return documentToString(chat.subscription ?? chat.query);
}

export { LOBBY_ROOM };
