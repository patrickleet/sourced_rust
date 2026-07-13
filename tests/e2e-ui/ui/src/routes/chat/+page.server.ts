import type { Actions, PageServerLoad } from './$types';
import { fail } from '@sveltejs/kit';
import { engineRoleFromGroups } from '../../auth';
import { serverCommand, serverGraphql } from '$lib/server/graphql';

type ChatMsg = {
  message_id: string;
  room_id: string;
  author_id: string;
  body: string;
  created_at: string;
};

const ROOM = 'lobby';

export const load: PageServerLoad = async ({ locals }) => {
  const session = await locals.auth();
  const accessToken = session?.accessToken;
  const role = engineRoleFromGroups(session?.user?.groups);

  const result = await serverGraphql<{ chat_messages: ChatMsg[] }>(
    `{ chat_messages(where: { room_id: { _eq: "${ROOM}" } }) { message_id room_id author_id body created_at } }`,
    {
      accessToken,
      userId: accessToken ? undefined : session?.user?.id,
      role
    }
  );

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

export const actions: Actions = {
  post: async ({ request, locals }) => {
    const session = await locals.auth();
    if (!session?.user) return fail(401, { message: 'unauthorized' });
    const fd = await request.formData();
    const body = String(fd.get('body') || '').trim();
    if (!body) return fail(400, { message: 'empty message' });
    const message_id = `m-${Date.now().toString(16)}`;
    const role = engineRoleFromGroups(session.user.groups);
    const res = await serverCommand(
      'chat.post',
      { message_id, body, room_id: ROOM },
      {
        accessToken: session.accessToken,
        userId: session.accessToken ? undefined : session.user.id,
        role
      }
    );
    if (!res.ok) return fail(res.status, { message: res.body?.error ?? 'post failed' });
    return { ok: true };
  }
};
