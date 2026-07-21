/** Bearer and DevHeaders mapping shared by HTTP and WebSocket transports. */
import type { GqlAuth } from './types.js';

/** HTTP headers for a JSON GraphQL request. */
export function buildAuthHeaders(auth: GqlAuth = {}): Record<string, string> {
	const headers: Record<string, string> = { 'content-type': 'application/json' };
	const token = auth.accessToken?.trim() ?? '';

	if (token) {
		headers.authorization = `Bearer ${token}`;
	} else if (auth.userId) {
		headers['x-user-id'] = auth.userId;
		headers['x-role'] = auth.role ?? 'user';
	}

	return headers;
}

/** `graphql-transport-ws` connection-init payload. */
export function wsConnectionInitPayload(auth: GqlAuth = {}): Record<string, string> {
	const payload: Record<string, string> = {};
	const token = auth.accessToken?.trim() ?? '';

	if (token) {
		payload.authorization = `Bearer ${token}`;
		payload.accessToken = token;
	} else if (auth.userId) {
		payload['x-user-id'] = auth.userId;
		payload['x-role'] = auth.role ?? 'user';
	}

	return payload;
}

/** Add DevHeaders to a WebSocket URL when bearer authentication is absent. */
export function applyWsDevHeaderParams(url: URL, auth: GqlAuth = {}): void {
	const token = auth.accessToken?.trim() ?? '';
	if (token || !auth.userId) return;

	url.searchParams.set('x-user-id', auth.userId);
	url.searchParams.set('x-role', auth.role ?? 'user');
}
