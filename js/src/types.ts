/** Shared GraphQL client types for browser and server runtimes. */
import type { GraphqlResponseExtensions } from './protocol.js';
import type { CausalCommandReceipt } from './causal.js';

/** Variables accepted by an untyped GraphQL operation. */
export type GraphqlVariables = Record<string, unknown>;

/** Standard source location attached to a GraphQL error. */
export type GqlErrorLocation = {
	line: number;
	column: number;
};

/** GraphQL execution or transport error returned by the server. */
export type GqlError = {
	message: string;
	locations?: GqlErrorLocation[];
	path?: Array<string | number>;
	extensions?: Record<string, unknown> & { code?: string };
};

/** Result returned by {@link requestGraphql} and {@link GraphqlClient.request}. */
export type GqlResult<TData> = {
	data?: TData;
	errors?: GqlError[];
	/** Validated GraphQL response extensions, including Distributed receipts. */
	extensions?: GraphqlResponseExtensions;
	/** Generated causal commands attach their recovery handle here. */
	receipt?: CausalCommandReceipt;
	status: number;
};

/** Authentication material understood by Distributed's GraphQL transports. */
export type GqlAuth = {
	accessToken?: string | null;
	/** DevHeaders offline fallback only; ignored when an access token is present. */
	userId?: string | null;
	role?: string | null;
};
