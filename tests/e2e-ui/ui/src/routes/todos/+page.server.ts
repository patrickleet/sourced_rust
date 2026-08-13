import type { PageServerLoad } from './$types';
import { requireAuth } from '$lib/server/require-auth';

export const load: PageServerLoad = async (event) => {
	await requireAuth(event);
	return {};
};
