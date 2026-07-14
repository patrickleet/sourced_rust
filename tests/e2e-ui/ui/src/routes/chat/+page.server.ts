import type { PageServerLoad } from './$types';
import { engineRoleFromGroups } from '$lib/roles';
import { chatMessagesQuery } from '$lib/gql/documents';
import { serverGraphql } from '$lib/server/graphql';

type ChatMsg = {
  message_id: string;
  room_id: string;
  author_id: string;
  body: string;
  created_at: string;
};

const ROOM = 'lobby';

/** SSR seed — same selection as browser subscription / posts. */
export const load: PageServerLoad = async ({ locals }) => {
  const session = await locals.auth();
  const accessToken = session?.accessToken;
  const role = engineRoleFromGroups(session?.user?.groups);

  const result = await serverGraphql<{ chat_messages: ChatMsg[] }>(chatMessagesQuery(ROOM), {
    accessToken,
    userId: accessToken ? undefined : session?.user?.id,
    role
  });

  const messages = [...(result.data?.chat_messages ?? [])].sort((a, b) =>
    a.created_at === b.created_at
      ? a.message_id.localeCompare(b.message_id)
      : a.created_at.localeCompare(b.created_at)
  );

  return {
    session,
    room: ROOM,
    messages,
    accessToken: accessToken ?? null,
    engineRole: role,
    userId: session?.user?.id ?? null,
    gqlError: result.errors?.[0]?.message ?? (result.status >= 400 ? `HTTP ${result.status}` : null)
  };
};
