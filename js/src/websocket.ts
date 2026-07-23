/** Minimal `graphql-transport-ws` client with injectable browser primitives. */
import {
	applyWsDevHeaderParams,
	wsConnectionInitPayload
} from './auth-headers.js';
import { documentToString, type GqlDocument } from './document.js';
import {
	DistributedProtocolError,
	distributedLiveResumeExtensions,
	parseGraphqlResponseExtensions,
	type DistributedLiveCursor,
	type GraphqlResponseExtensions
} from './protocol.js';
import type {
	GqlAuth,
	GqlError,
	GraphqlVariables
} from './types.js';

/** Constructor accepted by the WebSocket transport (native or test implementation). */
export type WebSocketConstructor = typeof globalThis.WebSocket;

/** GraphQL execution payload delivered by a subscription. */
export type GqlWsResult<TData = unknown> = {
	data?: TData | null;
	errors?: GqlError[];
	/** Validated top-level GraphQL extensions for this live frame. */
	extensions?: GraphqlResponseExtensions;
};

export type GqlWsHandlers<TData = unknown> = {
	onNext: (result: GqlWsResult<TData>) => void;
	onError?: (error: unknown) => void;
	onComplete?: () => void;
};

export type SubscribeOptions<
	TVariables extends GraphqlVariables = GraphqlVariables
> = {
	/** HTTP GraphQL URL used to derive the `.../ws` endpoint. */
	httpUrl?: string;
	/** Variables for the subscription operation. */
	variables?: TVariables;
	/**
	 * Latest server-issued cursors for a generated live operation.
	 * @internal Application code must not synthesize resume tokens.
	 */
	resume?: readonly DistributedLiveCursor[];
	/** Override the runtime's global WebSocket constructor. */
	webSocket?: WebSocketConstructor;
};

function browserLocation(): Location | undefined {
	return typeof window === 'undefined' ? undefined : window.location;
}

/** Build a same-origin WebSocket URL in a browser, or retain the path on a server. */
export function graphqlWsUrl(path = '/graphql/ws'): string {
	const location = browserLocation();
	if (!location) return path;

	const protocol = location.protocol === 'https:' ? 'wss:' : 'ws:';
	const normalizedPath = path.startsWith('/') ? path : `/${path}`;
	return `${protocol}//${location.host}${normalizedPath}`;
}

/** Map an HTTP GraphQL URL to its `graphql-transport-ws` endpoint. */
export function httpUrlToWsUrl(httpUrl: string): string {
	const location = browserLocation();
	const base = location?.href ?? 'http://127.0.0.1/';

	try {
		const url = new URL(httpUrl, base);
		const path = url.pathname.replace(/\/$/, '');
		const wsPath = path.endsWith('/ws') ? path : `${path}/ws`;

		if (httpUrl.startsWith('/')) {
			return location
				? graphqlWsUrl(wsPath)
				: `ws://127.0.0.1${wsPath.startsWith('/') ? wsPath : `/${wsPath}`}`;
		}

		url.protocol = url.protocol === 'https:' || url.protocol === 'wss:' ? 'wss:' : 'ws:';
		url.pathname = wsPath;
		url.search = '';
		url.hash = '';
		return url.toString();
	} catch {
		return location ? graphqlWsUrl('/graphql/ws') : 'ws://127.0.0.1/graphql/ws';
	}
}

/** Open one GraphQL subscription and return an idempotent unsubscribe function. */
export function subscribe<
	TData = unknown,
	TVariables extends GraphqlVariables = GraphqlVariables
>(
	document: GqlDocument<TData, TVariables>,
	auth: GqlAuth = {},
	handlers: GqlWsHandlers<TData>,
	options: SubscribeOptions<TVariables> = {}
): () => void {
	const WebSocketImpl = options.webSocket ?? globalThis.WebSocket;
	if (typeof WebSocketImpl !== 'function') {
		throw new Error(
			'subscribe requires a WebSocket implementation in this runtime; pass { webSocket } in the options'
		);
	}

	const query = documentToString(document);
	const location = browserLocation();
	const href = options.httpUrl
		? httpUrlToWsUrl(options.httpUrl)
		: graphqlWsUrl('/graphql/ws');
	const url = new URL(href, location?.href ?? 'ws://127.0.0.1/');
	applyWsDevHeaderParams(url, auth);

	const socket = new WebSocketImpl(url.toString(), 'graphql-transport-ws');
	const operationId = '1';
	let closed = false;

	const rejectProtocolFrame = (error: unknown) => {
		handlers.onError?.(error);
		if (closed) return;
		closed = true;
		if (socket.readyState === WebSocketImpl.OPEN) {
			try {
				socket.send(JSON.stringify({ type: 'complete', id: operationId }));
			} catch {
				// The protocol is already rejected; closing is the remaining fence.
			}
		}
		socket.close();
	};

	socket.onopen = () => {
		socket.send(
			JSON.stringify({
				type: 'connection_init',
				payload: wsConnectionInitPayload(auth)
			})
		);
	};

	socket.onmessage = (event) => {
		let message: { type: string; id?: string; payload?: unknown };
		try {
			message = JSON.parse(String(event.data)) as typeof message;
		} catch {
			return;
		}

		switch (message.type) {
			case 'connection_ack':
				const extensions =
					options.resume === undefined || options.resume.length === 0
						? undefined
						: distributedLiveResumeExtensions(options.resume);
				socket.send(
					JSON.stringify({
						type: 'subscribe',
						id: operationId,
						payload: {
							query,
							variables: options.variables ?? {},
							...(extensions === undefined ? {} : { extensions })
						}
					})
				);
				break;
			case 'next':
				try {
					handlers.onNext(parseNextPayload<TData>(message.payload));
				} catch (error) {
					rejectProtocolFrame(error);
				}
				break;
			case 'error':
				handlers.onError?.(message.payload ?? 'subscription error');
				break;
			case 'complete':
				handlers.onComplete?.();
				break;
			case 'ping':
				socket.send(JSON.stringify({ type: 'pong' }));
				break;
			case 'connection_error':
				handlers.onError?.(message.payload ?? 'connection error');
				break;
			default:
				break;
		}
	};

	socket.onerror = (event) => handlers.onError?.(event);
	socket.onclose = (event) => {
		if (!closed && !event.wasClean) {
			handlers.onError?.(
				`WebSocket closed (${event.code}${event.reason ? `: ${event.reason}` : ''}) — check the GraphQL WebSocket endpoint`
			);
		}
		if (!closed) handlers.onComplete?.();
	};

	return () => {
		if (closed) return;
		closed = true;
		if (socket.readyState === WebSocketImpl.OPEN) {
			try {
				socket.send(JSON.stringify({ type: 'complete', id: operationId }));
			} catch {
				// The transport may have closed between the ready-state check and send.
			}
		}
		socket.close();
	};
}

function parseNextPayload<TData>(value: unknown): GqlWsResult<TData> {
	if (value === null || typeof value !== 'object' || Array.isArray(value)) {
		throw new DistributedProtocolError(
			'DISTRIBUTED_PROTOCOL_INVALID',
			'websocket.next.payload'
		);
	}
	const payload = value as Record<string, unknown>;
	if (payload.errors !== undefined && !Array.isArray(payload.errors)) {
		throw new DistributedProtocolError(
			'DISTRIBUTED_PROTOCOL_INVALID',
			'websocket.next.payload.errors'
		);
	}
	const extensions = parseGraphqlResponseExtensions(payload.extensions);
	return {
		...(payload as GqlWsResult<TData>),
		...(extensions === undefined ? {} : { extensions })
	};
}
