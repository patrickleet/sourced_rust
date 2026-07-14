/**
 * graphql-transport-ws client.
 * Auth: Bearer in connection_init (same GqlAuth mapping as HTTP via auth-headers).
 */
import {
	applyWsDevHeaderParams,
	wsConnectionInitPayload
} from '$lib/gql/auth-headers';
import { documentToString, type GqlDocument } from '$lib/gql/document';
import type { GqlAuth } from '$lib/gql/types';

export type GqlWsHandlers = {
	onNext: (data: unknown) => void;
	onError?: (err: unknown) => void;
	onComplete?: () => void;
};

export function graphqlWsUrl(path = '/graphql/ws'): string {
	if (typeof window === 'undefined') return path;
	const proto = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
	// Prefer same-origin so Vite proxies WS in dev.
	return `${proto}//${window.location.host}${path.startsWith('/') ? path : `/${path}`}`;
}

export function subscribe(
	document: GqlDocument,
	auth: GqlAuth = {},
	handlers: GqlWsHandlers
): () => void {
	const query = documentToString(document);
	const url = new URL(graphqlWsUrl('/graphql/ws'), window.location.href);
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
						payload: { query }
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
