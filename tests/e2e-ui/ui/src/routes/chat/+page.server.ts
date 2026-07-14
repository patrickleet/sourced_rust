import { loadQuery } from '$lib/gql/load-query.server';
import { chat, LOBBY_ROOM, sortChatMessages } from './chat.resource';
import type { ChatMsg, ChatQueryData } from './chat.resource';

/**
 * SSR seed — same `chat.query` selection the browser subscription uses.
 * Posts run in the browser via useGraphql → POST /graphql.
 */
export const load = loadQuery<ChatQueryData, { room: string; messages: ChatMsg[] }>(
	chat.query,
	(data) => ({
		room: LOBBY_ROOM,
		messages: sortChatMessages(data?.chat_messages ?? [])
	})
);
