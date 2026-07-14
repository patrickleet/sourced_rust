import type { Actions, PageServerLoad } from './$types';
import { fail } from '@sveltejs/kit';
import { engineRoleFromGroups } from '$lib/roles';
import { serverCommand, serverGraphql } from '$lib/server/graphql';

type Todo = {
	todo_id: string;
	owner_id: string;
	title: string;
	status: string;
};

/** Client-generated ids are preferred for optimistic UI; fall back for no-JS. */
function todoIdFromForm(fd: FormData): string {
	const raw = String(fd.get('todo_id') || '').trim();
	if (/^t-[a-zA-Z0-9_-]{4,40}$/.test(raw)) return raw;
	return `t-${Date.now().toString(16)}`;
}

export const load: PageServerLoad = async ({ locals }) => {
	const session = await locals.auth();
	const accessToken = session?.accessToken;
	const role = engineRoleFromGroups(session?.user?.groups);

	const result = await serverGraphql<{ todos: Todo[] }>(
		`{
			todos {
				todo_id
				owner_id
				title
				status
			}
		}`,
		{
			accessToken,
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
		const todo_id = todoIdFromForm(fd);
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
		if (!res.ok) {
			return fail(res.status, {
				message: (res.body as { error?: string })?.error ?? 'create failed',
				todo_id
			});
		}
		return { ok: true as const, todo_id, title };
	},

	complete: async ({ request, locals }) => {
		const session = await locals.auth();
		if (!session?.user) return fail(401, { message: 'unauthorized' });
		const fd = await request.formData();
		const todo_id = String(fd.get('todo_id') || '');
		if (!todo_id) return fail(400, { message: 'todo_id required' });
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
		if (!res.ok) {
			return fail(res.status, {
				message: (res.body as { error?: string })?.error ?? 'complete failed',
				todo_id
			});
		}
		return { ok: true as const, todo_id };
	},

	archive: async ({ request, locals }) => {
		const session = await locals.auth();
		if (!session?.user) return fail(401, { message: 'unauthorized' });
		const fd = await request.formData();
		const todo_id = String(fd.get('todo_id') || '');
		if (!todo_id) return fail(400, { message: 'todo_id required' });
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
		if (!res.ok) {
			return fail(res.status, {
				message: (res.body as { error?: string })?.error ?? 'archive failed',
				todo_id
			});
		}
		return { ok: true as const, todo_id };
	}
};
