/**
 * Co-located lobby chat query + subscription — documents from `chat.gql`.
 * Posts: `$lib/api/commands.generated` → `chatMessagesPost`.
 */
import { defineResource } from '$lib/gql/define-resource';
import {
	ChatMessagesDocument,
	ChatMessagesLiveDocument,
	type ChatMessagesQuery
} from './chat.generated';

export type ChatMsg = ChatMessagesQuery['chat_messages'][number];
export type ChatQueryData = ChatMessagesQuery;

/** Default room for the e2e-ui lobby page (matches chat.gql filter). */
export const LOBBY_ROOM = 'lobby';

export function sortChatMessages(list: ChatMsg[]): ChatMsg[] {
	return [...list].sort((a, b) =>
		a.created_at === b.created_at
			? a.message_id.localeCompare(b.message_id)
			: a.created_at.localeCompare(b.created_at)
	);
}

/** Lobby resource used by `routes/chat` (SSR seed + live sub). */
export const chat = defineResource<ChatQueryData>({
	query: ChatMessagesDocument,
	subscription: ChatMessagesLiveDocument,
	select: (data) => sortChatMessages(data.chat_messages ?? [])
});
