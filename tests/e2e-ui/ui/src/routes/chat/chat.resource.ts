/**
 * Co-located lobby chat GraphQL ops — query, subscription, and post mutation.
 * SSR load, WS subscribe, and browser post share documents from this module.
 */
import { defineResource } from '$lib/gql/define-resource';

export type ChatMsg = {
	message_id: string;
	room_id: string;
	author_id: string;
	body: string;
	created_at: string;
};

export type ChatQueryData = {
	chat_messages: ChatMsg[];
};

/** Default room for the e2e-ui lobby page. */
export const LOBBY_ROOM = 'lobby';

const CHAT_POST = `mutation ChatPost($message_id: String!, $body: String!, $room_id: String!) {
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

const messageSelection = `message_id
    room_id
    author_id
    body
    created_at`;

function chatMessagesQuery(room: string): string {
	return `{
  chat_messages(where: { room_id: { _eq: "${room}" } }) {
    ${messageSelection}
  }
}`;
}

function chatMessagesSubscription(room: string): string {
	return `subscription {
  chat_messages(where: { room_id: { _eq: "${room}" } }) {
    ${messageSelection}
  }
}`;
}

export function sortChatMessages(list: ChatMsg[]): ChatMsg[] {
	return [...list].sort((a, b) =>
		a.created_at === b.created_at
			? a.message_id.localeCompare(b.message_id)
			: a.created_at.localeCompare(b.created_at)
	);
}

/**
 * Build a chat resource for a room. Query + subscription share the same
 * selection set; post mutation takes room_id as a variable.
 */
export function chatResource(room: string) {
	return defineResource<ChatQueryData, { post: string }>({
		query: chatMessagesQuery(room),
		subscription: chatMessagesSubscription(room),
		mutations: {
			post: CHAT_POST
		},
		select: (data) => sortChatMessages(data.chat_messages ?? [])
	});
}

/** Lobby resource used by `routes/chat` (SSR seed + live sub + post). */
export const chat = chatResource(LOBBY_ROOM);
