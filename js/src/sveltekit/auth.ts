import type { GqlAuth } from '../types.js';

/** Minimal page-data shape needed to bind browser GraphQL authentication. */
export type PageGraphqlData = {
	accessToken?: string | null;
	session?: {
		accessToken?: string | null;
		user?: {
			id?: string | null;
			name?: string | null;
			email?: string | null;
			username?: string | null;
			groups?: string[];
		} | null;
	} | null;
	engineRole?: string | null;
};

/** Map SSR/page data into GraphQL auth (Bearer preferred; DevHeaders for local development). */
export function authFromPageData(data: PageGraphqlData): GqlAuth {
	const accessToken = data.accessToken ?? data.session?.accessToken ?? null;
	return {
		accessToken,
		userId: accessToken ? undefined : (data.session?.user?.id ?? undefined),
		role: data.engineRole ?? undefined
	};
}
