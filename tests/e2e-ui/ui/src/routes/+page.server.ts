import type { PageServerLoad } from './$types';

/** Home is static template content; session comes from layout. */
export const load: PageServerLoad = async () => {
	return {};
};
