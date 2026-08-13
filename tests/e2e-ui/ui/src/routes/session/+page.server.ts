import type { PageServerLoad } from './$types';
import { requireAuth } from '$lib/server/require-auth';

export const load: PageServerLoad = async (event) => {
	const session = await requireAuth(event);
	return { session };
};
