import type { Actions, PageServerLoad } from './$types';
import { fail } from '@sveltejs/kit';
import { engineRoleFromGroups } from '$lib/roles';
import { serverGraphql } from '$lib/server/graphql';

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

function authOpts(session: {
	accessToken?: string | null;
	user?: { id?: string; groups?: string[] };
}) {
	const accessToken = session.accessToken;
	return {
		accessToken,
		userId: accessToken ? undefined : session.user?.id,
		role: engineRoleFromGroups(session.user?.groups)
	};
}

function gqlFail(
	result: { errors?: Array<{ message: string }>; status: number },
	fallback: string,
	extra: Record<string, string> = {}
) {
	return fail(result.status >= 400 ? result.status : 400, {
		message: result.errors?.[0]?.message ?? fallback,
		...extra
	});
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
		const result = await serverGraphql<{
			todos_create?: { todo_id: string; owner_id: string; title: string; status: string };
		}>(
			`mutation TodosCreate($todo_id: String!, $title: String!) {
				todos_create(input: { todo_id: $todo_id, title: $title }) {
					todo_id
					owner_id
					title
					status
				}
			}`,
			{ ...authOpts(session), variables: { todo_id, title } }
		);
		if (result.errors?.length || !result.data?.todos_create) {
			return gqlFail(result, 'create failed', { todo_id });
		}
		return {
			ok: true as const,
			todo_id: result.data.todos_create.todo_id,
			title: result.data.todos_create.title
		};
	},

	complete: async ({ request, locals }) => {
		const session = await locals.auth();
		if (!session?.user) return fail(401, { message: 'unauthorized' });
		const fd = await request.formData();
		const todo_id = String(fd.get('todo_id') || '');
		if (!todo_id) return fail(400, { message: 'todo_id required' });
		const result = await serverGraphql<{ todos_complete?: { todo_id: string; status: string } }>(
			`mutation TodosComplete($todo_id: String!) {
				todos_complete(input: { todo_id: $todo_id }) {
					todo_id
					status
				}
			}`,
			{ ...authOpts(session), variables: { todo_id } }
		);
		if (result.errors?.length || !result.data?.todos_complete) {
			return gqlFail(result, 'complete failed', { todo_id });
		}
		return { ok: true as const, todo_id };
	},

	archive: async ({ request, locals }) => {
		const session = await locals.auth();
		if (!session?.user) return fail(401, { message: 'unauthorized' });
		const fd = await request.formData();
		const todo_id = String(fd.get('todo_id') || '');
		if (!todo_id) return fail(400, { message: 'todo_id required' });
		const result = await serverGraphql<{ todos_archive?: { todo_id: string; status: string } }>(
			`mutation TodosArchive($todo_id: String!) {
				todos_archive(input: { todo_id: $todo_id }) {
					todo_id
					status
				}
			}`,
			{ ...authOpts(session), variables: { todo_id } }
		);
		if (result.errors?.length || !result.data?.todos_archive) {
			return gqlFail(result, 'archive failed', { todo_id });
		}
		return { ok: true as const, todo_id };
	}
};
