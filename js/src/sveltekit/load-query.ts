import type { GqlDocument } from '../document.js';
import type { GqlAuth, GqlResult } from '../types.js';
import { authFromPageData, type PageGraphqlData } from './auth.js';

export type SveltekitSession = NonNullable<PageGraphqlData['session']>;

/** Structural SvelteKit server-load boundary; avoids coupling consumers to our dev dependency graph. */
export type ServerLoadEventLike<TLocals = unknown> = {
	locals: TLocals;
};

export type LoadQueryRequest = <TData>(
	document: GqlDocument,
	auth: GqlAuth,
	variables?: Record<string, unknown>
) => Promise<GqlResult<TData>>;

export type CreateLoadQueryOptions<
	TSession extends SveltekitSession,
	TEvent extends ServerLoadEventLike = ServerLoadEventLike
> = {
	getSession: (event: TEvent) => Promise<TSession | null>;
	getRole: (session: TSession | null, event: TEvent) => string | null | undefined;
	request: LoadQueryRequest;
	/** Override the standard accessToken/session-user mapping. */
	getAuth?: (pageData: PageGraphqlData, event: TEvent) => GqlAuth;
};

/**
 * Inject app-owned session, role, and private API-origin wiring once; the
 * returned helper creates typed SSR loads with consistent browser page data.
 */
export function createLoadQuery<
	TSession extends SveltekitSession,
	TEvent extends ServerLoadEventLike = ServerLoadEventLike
>(
	options: CreateLoadQueryOptions<TSession, TEvent>
) {
	return function loadQuery<TData, TMapped extends Record<string, unknown>>(
		document: GqlDocument,
		map: (data: TData | undefined, result: GqlResult<TData>) => TMapped,
		variables?: Record<string, unknown>
	) {
		return async (event: TEvent) => {
			const session = await options.getSession(event);
			const accessToken = session?.accessToken ?? null;
			const engineRole = options.getRole(session, event) ?? null;
			const pageData: PageGraphqlData = { session, accessToken, engineRole };
			const auth = options.getAuth?.(pageData, event) ?? authFromPageData(pageData);
			const result = await options.request<TData>(document, auth, variables);

			return {
				session,
				accessToken,
				engineRole,
				gqlError:
					result.errors?.[0]?.message ??
					(result.status >= 400 ? `HTTP ${result.status}` : null),
				gqlStatus: result.status,
				...map(result.data, result)
			};
		};
	};
}
