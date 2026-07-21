/** Authentication identity helpers used to isolate client-side caches. */
import type { GqlAuth } from './types.js';

/**
 * Produce a stable cache identity without embedding a bearer token in cache keys.
 *
 * JWTs use the unverified `sub` claim so token refreshes retain the same cache.
 * Opaque bearer tokens use a non-cryptographic hash of the complete token.
 * DevHeaders use the user and role pair.
 */
export function authIdentityKey(auth: GqlAuth): string {
	const token = auth.accessToken?.trim() ?? '';
	if (token) {
		const subject = jwtPayloadSub(token);
		return subject ? `sub:${subject}` : `bearer:${hashString(token)}`;
	}

	return `dev:${auth.userId ?? ''}:${auth.role ?? ''}`;
}

/** Decode an unverified JWT payload's `sub` claim for UI cache identity only. */
export function jwtPayloadSub(token: string): string | null {
	const parts = token.split('.');
	if (parts.length !== 3 || !parts[1]) return null;

	try {
		const payload = JSON.parse(base64UrlDecode(parts[1])) as { sub?: unknown };
		return typeof payload.sub === 'string' && payload.sub.length > 0 ? payload.sub : null;
	} catch {
		return null;
	}
}

function base64UrlDecode(segment: string): string {
	if (typeof globalThis.atob !== 'function') {
		throw new Error('base64 decoding is unavailable in this runtime');
	}

	const padding = segment.length % 4 === 0 ? '' : '='.repeat(4 - (segment.length % 4));
	const binary = globalThis.atob(segment.replace(/-/g, '+').replace(/_/g, '/') + padding);
	const bytes = Uint8Array.from(binary, (character) => character.charCodeAt(0));
	return new TextDecoder().decode(bytes);
}

/** FNV-1a 32-bit: stable obfuscation for opaque tokens, not a security primitive. */
function hashString(value: string): string {
	let hash = 0x811c9dc5;
	for (let index = 0; index < value.length; index += 1) {
		hash ^= value.charCodeAt(index);
		hash = Math.imul(hash, 0x01000193);
	}
	return (hash >>> 0).toString(16);
}
