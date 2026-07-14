/**
 * Pure page-data → GqlAuth mapping (no createGraphqlClient import).
 * Safe for unit tests under node --experimental-strip-types.
 */
import type { GqlAuth } from './types';

export type PageGraphqlData = {
	accessToken?: string | null;
	session?: {
		accessToken?: string | null;
		user?: { id?: string | null } | null;
	} | null;
	engineRole?: string | null;
};

/** Map SSR/page data into GraphQL auth headers (Bearer preferred; DevHeaders offline). */
export function authFromPageData(data: PageGraphqlData): GqlAuth {
	const accessToken = data.accessToken ?? data.session?.accessToken ?? null;
	return {
		accessToken,
		userId: accessToken ? undefined : (data.session?.user?.id ?? undefined),
		role: data.engineRole ?? undefined
	};
}
