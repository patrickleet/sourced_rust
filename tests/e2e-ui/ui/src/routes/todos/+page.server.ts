import type { Actions, PageServerLoad } from './$types';
import { fail } from '@sveltejs/kit';
import { engineRoleFromGroups } from '../../auth';
import { serverCommand, serverGraphql } from '$lib/server/graphql';

type Todo = {
  todo_id: string;
  owner_id: string;
  title: string;
  status: string;
};

export const load: PageServerLoad = async ({ locals }) => {
  const session = await locals.auth();
  const accessToken = session?.accessToken;
  const role = engineRoleFromGroups(session?.user?.groups);

  // SSR: fetch list with Bearer so HTML includes data (no client Loading flash).
  const result = await serverGraphql<{ todos: Todo[] }>(
    `{ todos { todo_id owner_id title status } }`,
    {
      accessToken,
      // offline DevHeaders fallback when no OIDC session token
      userId: accessToken ? undefined : session?.user?.id,
      role
    }
  );

  return {
    session,
    todos: result.data?.todos ?? [],
    gqlError: result.errors?.[0]?.message ?? (result.status >= 400 ? `HTTP ${result.status}` : null),
    gqlStatus: result.status
  };
};

export const actions: Actions = {
  create: async ({ request, locals }) => {
    const session = await locals.auth();
    if (!session?.user) return fail(401, { message: 'unauthorized' });
    const fd = await request.formData();
    const title = String(fd.get('title') || '').trim();
    if (!title) return fail(400, { message: 'title required' });
    const todo_id = `t-${Date.now().toString(16)}`;
    const role = engineRoleFromGroups(session.user.groups);
    const res = await serverCommand(
      'todo.create',
      { todo_id, title },
      {
        accessToken: session.accessToken,
        userId: session.accessToken ? undefined : session.user.id,
        role
      }
    );
    if (!res.ok) return fail(res.status, { message: res.body?.error ?? 'create failed' });
    return { ok: true, todo_id };
  },
  complete: async ({ request, locals }) => {
    const session = await locals.auth();
    if (!session?.user) return fail(401, { message: 'unauthorized' });
    const fd = await request.formData();
    const todo_id = String(fd.get('todo_id') || '');
    const role = engineRoleFromGroups(session.user.groups);
    const res = await serverCommand(
      'todo.complete',
      { todo_id },
      {
        accessToken: session.accessToken,
        userId: session.accessToken ? undefined : session.user.id,
        role
      }
    );
    if (!res.ok) return fail(res.status, { message: res.body?.error ?? 'complete failed' });
    return { ok: true };
  },
  archive: async ({ request, locals }) => {
    const session = await locals.auth();
    if (!session?.user) return fail(401, { message: 'unauthorized' });
    const fd = await request.formData();
    const todo_id = String(fd.get('todo_id') || '');
    const role = engineRoleFromGroups(session.user.groups);
    const res = await serverCommand(
      'todo.archive',
      { todo_id },
      {
        accessToken: session.accessToken,
        userId: session.accessToken ? undefined : session.user.id,
        role
      }
    );
    if (!res.ok) return fail(res.status, { message: res.body?.error ?? 'archive failed' });
    return { ok: true };
  }
};
