export type DistributedGraphqlProxyOptions = {
	/** Absolute Distributed API origin, for example `http://127.0.0.1:8791`. */
	target: string;
	/** Proxy key. Defaults to `/graphql` and includes its `/ws` child. */
	path?: string;
};

/** Vite proxy config for GraphQL HTTP and WebSocket traffic. */
export function distributedGraphqlProxy(
	options: DistributedGraphqlProxyOptions | string
): Record<string, { target: string; changeOrigin: true; ws: true }> {
	const target = (typeof options === 'string' ? options : options.target).trim().replace(/\/$/, '');
	const path = typeof options === 'string' ? '/graphql' : (options.path ?? '/graphql');
	if (!/^https?:\/\//.test(target)) {
		throw new Error('distributedGraphqlProxy target must be an absolute http(s) URL');
	}
	if (!path.startsWith('/')) {
		throw new Error('distributedGraphqlProxy path must start with /');
	}
	return { [path]: { target, changeOrigin: true, ws: true } };
}
