/** Document-keyed query cache. Projected read models remain the source of truth. */
import type { GraphqlVariables } from '../types.js';

export type CacheKey = string;

export type CacheEntry<TData = unknown> = {
	data: TData;
	updatedAt: number;
	optimistic?: boolean;
	/** A command succeeded but its projection has not reconciled yet. */
	pending?: boolean;
};

export type CacheListener = () => void;

export class QueryCache {
	readonly #entries = new Map<CacheKey, CacheEntry<unknown>>();
	readonly #listeners = new Map<CacheKey, Set<CacheListener>>();
	#generation = 0;

	/**
	 * Monotonic ownership generation for asynchronous cache writers.
	 *
	 * A full clear advances the generation even when the cache is already empty,
	 * so work started for a previous authentication identity cannot repopulate it.
	 */
	get generation(): number {
		return this.#generation;
	}

	get<TData = unknown>(key: CacheKey): CacheEntry<TData> | undefined {
		return this.#entries.get(key) as CacheEntry<TData> | undefined;
	}

	set<TData>(key: CacheKey, entry: CacheEntry<TData>): void {
		this.#entries.set(key, entry as CacheEntry<unknown>);
		this.#notify(key);
	}

	update<TData>(key: CacheKey, update: (data: TData) => TData): void {
		const previous = this.get<TData>(key);
		if (!previous) return;

		this.set(key, {
			...previous,
			data: update(previous.data),
			updatedAt: Date.now()
		});
	}

	/** Delete exactly one cache key. */
	delete(key: CacheKey): void {
		if (this.#entries.delete(key)) this.#notify(key);
	}

	/** Delete one exact key, or every entry when called without a key. */
	invalidate(key?: CacheKey): void {
		if (key !== undefined) {
			this.delete(key);
			return;
		}

		const keys = [...this.#entries.keys()];
		this.#entries.clear();
		for (const entryKey of keys) this.#notify(entryKey);
	}

	/** Deliberately delete every key beginning with `prefix`. */
	invalidatePrefix(prefix: string): void {
		for (const key of [...this.#entries.keys()]) {
			if (key.startsWith(prefix)) this.delete(key);
		}
	}

	/** Drop all entries, for example after an authentication identity change. */
	clear(): void {
		this.#generation += 1;
		this.invalidate();
	}

	subscribe(key: CacheKey, listener: CacheListener): () => void {
		let listeners = this.#listeners.get(key);
		if (!listeners) {
			listeners = new Set();
			this.#listeners.set(key, listeners);
		}
		listeners.add(listener);

		return () => {
			listeners?.delete(listener);
			if (listeners?.size === 0) this.#listeners.delete(key);
		};
	}

	#notify(key: CacheKey): void {
		for (const listener of this.#listeners.get(key) ?? []) listener();
	}
}

/** Stable key for a normalized document string and GraphQL variables. */
export function cacheKey(document: string, variables?: GraphqlVariables): CacheKey {
	const serializedVariables =
		variables && Object.keys(variables).length > 0 ? stableStringify(variables) : '';
	return `${document.trim()}::${serializedVariables}`;
}

function stableStringify(value: unknown, ancestors = new Set<object>()): string {
	if (value === null || typeof value !== 'object') {
		const serialized = JSON.stringify(value);
		return serialized === undefined ? 'undefined' : serialized;
	}

	if (ancestors.has(value)) {
		throw new TypeError('GraphQL variables must not contain circular references');
	}
	ancestors.add(value);

	let serialized: string;
	if (Array.isArray(value)) {
		serialized = `[${value.map((item) => stableStringify(item, ancestors)).join(',')}]`;
	} else {
		const record = value as Record<string, unknown>;
		serialized = `{${Object.keys(record)
			.sort()
			.map((key) => `${JSON.stringify(key)}:${stableStringify(record[key], ancestors)}`)
			.join(',')}}`;
	}

	ancestors.delete(value);
	return serialized;
}
