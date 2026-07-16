/**
 * Small helpers for pages that seed / read a document-keyed QueryCache.
 */
import type { QueryCache } from './cache/query-cache.ts';
import { cacheKey } from './cache/query-cache.ts';
import { documentToString, type GqlDocument } from './document.ts';

export function seedQueryCache<T>(
	cache: QueryCache,
	document: GqlDocument,
	data: T,
	variables?: Record<string, unknown>
): string {
	const key = cacheKey(documentToString(document), variables);
	cache.set(key, {
		data,
		updatedAt: Date.now(),
		optimistic: false,
		pending: false
	});
	return key;
}

export function readQueryList<T>(
	cache: QueryCache,
	document: GqlDocument,
	at: string,
	variables?: Record<string, unknown>
): T[] {
	const key = cacheKey(documentToString(document), variables);
	const entry = cache.get<Record<string, unknown>>(key);
	const list = entry?.data?.[at];
	return Array.isArray(list) ? (list as T[]) : [];
}

export function queryDocString(document: GqlDocument): string {
	return documentToString(document);
}

/** List target for optimistic upserts / projection apply. */
export function listTarget(
	document: GqlDocument,
	at: string,
	by: string,
	variables?: Record<string, unknown>
) {
	return {
		document: documentToString(document),
		variables,
		at,
		by
	};
}
