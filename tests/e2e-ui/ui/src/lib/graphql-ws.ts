/**
 * graphql-transport-ws client.
 * Prefer `createGraphqlClient(...).subscribe` so auth/URL match HTTP.
 * Low-level `subscribe` still accepts explicit GqlAuth for tests.
 */
import {
	applyWsDevHeaderParams,
	wsConnectionInitPayload
} from './gql/auth-headers.ts';
import { documentToString, type GqlDocument } from './gql/document.ts';
import type { GqlAuth } from './gql/types.ts';

export type GqlWsHandlers = {
	onNext: (data: unknown) => void;
	onError?: (err: unknown) => void;
	onComplete?: () => void;
};

export type SubscribeOptions = {
	/**
	 * HTTP GraphQL URL from the bound client (`/graphql` or absolute).
	 * Used to derive the WebSocket endpoint (`…/graphql/ws`).
	 */
	httpUrl?: string;
	/** GraphQL variables for the subscription operation (must match document-store cache key). */
	variables?: Record<string, unknown>;
};

/** Same-origin WS path (Vite proxies `/graphql` including `/graphql/ws`). */
export function graphqlWsUrl(path = '/graphql/ws'): string {
	if (typeof window === 'undefined') return path;
	const proto = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
	return `${proto}//${window.location.host}${path.startsWith('/') ? path : `/${path}`}`;
}

/**
 * Map HTTP GraphQL URL → WebSocket subscription URL.
 * `/graphql` → same-origin `/graphql/ws`
 * `http://host:8791/graphql` → `ws://host:8791/graphql/ws`
 */
export function httpUrlToWsUrl(httpUrl: string): string {
	const base =
		typeof window !== 'undefined' ? window.location.href : 'http://127.0.0.1/';
	try {
		const u = new URL(httpUrl, base);
		const path = u.pathname.replace(/\/$/, '');
		const wsPath = path.endsWith('/ws') ? path : `${path}/ws`;
		// Relative HTTP paths → same-origin WS (browser) or absolute ws URL (SSR/tests).
		if (httpUrl.startsWith('/')) {
			if (typeof window !== 'undefined') return graphqlWsUrl(wsPath);
			return `ws://127.0.0.1${wsPath.startsWith('/') ? wsPath : `/${wsPath}`}`;
		}
		u.protocol = u.protocol === 'https:' ? 'wss:' : 'ws:';
		u.pathname = wsPath;
		u.search = '';
		u.hash = '';
		return u.toString();
	} catch {
		return typeof window !== 'undefined'
			? graphqlWsUrl('/graphql/ws')
			: 'ws://127.0.0.1/graphql/ws';
	}
}

export function subscribe(
	document: GqlDocument,
	auth: GqlAuth = {},
	handlers: GqlWsHandlers,
	options: SubscribeOptions = {}
): () => void {
	const query = documentToString(document);
	const wsHref = options.httpUrl
		? httpUrlToWsUrl(options.httpUrl)
		: graphqlWsUrl('/graphql/ws');
	const url = new URL(wsHref, typeof window !== 'undefined' ? window.location.href : wsHref);
	applyWsDevHeaderParams(url, auth);

	const ws = new WebSocket(url.toString(), 'graphql-transport-ws');
	const opId = '1';
	let closed = false;

	ws.onopen = () => {
		ws.send(
			JSON.stringify({
				type: 'connection_init',
				payload: wsConnectionInitPayload(auth)
			})
		);
	};

	ws.onmessage = (ev) => {
		let msg: { type: string; id?: string; payload?: unknown };
		try {
			msg = JSON.parse(String(ev.data));
		} catch {
			return;
		}
		switch (msg.type) {
			case 'connection_ack':
				ws.send(
					JSON.stringify({
						type: 'subscribe',
						id: opId,
						payload: {
							query,
							variables: options.variables ?? {}
						}
					})
				);
				break;
			case 'next':
				handlers.onNext(msg.payload);
				break;
			case 'error':
				handlers.onError?.(msg.payload ?? 'subscription error');
				break;
			case 'complete':
				handlers.onComplete?.();
				break;
			case 'ping':
				ws.send(JSON.stringify({ type: 'pong' }));
				break;
			case 'connection_error':
				handlers.onError?.(msg.payload ?? 'connection_error');
				break;
			default:
				break;
		}
	};

	ws.onerror = (e) => handlers.onError?.(e);
	ws.onclose = (ev) => {
		if (!closed && !ev.wasClean) {
			handlers.onError?.(
				`WebSocket closed (${ev.code}${ev.reason ? `: ${ev.reason}` : ''}) — check API /graphql/ws`
			);
		}
		if (!closed) handlers.onComplete?.();
	};

	return () => {
		closed = true;
		if (ws.readyState === WebSocket.OPEN) {
			try {
				ws.send(JSON.stringify({ type: 'complete', id: opId }));
			} catch {
				/* ignore */
			}
		}
		ws.close();
	};
}
