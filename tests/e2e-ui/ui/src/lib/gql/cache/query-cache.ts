/**
 * Document-keyed browser query cache (pure).
 * Not a system of truth — projectors + live queries own freshness.
 */

export type CacheKey = string;

export type CacheEntry<T = unknown> = {
  data: T;
  updatedAt: number;
  optimistic?: boolean;
  /** Command succeeded; projection not yet reconciled. */
  pending?: boolean;
};

export type CacheListener = () => void;

export class QueryCache {
  #entries = new Map<CacheKey, CacheEntry>();
  #listeners = new Map<CacheKey, Set<CacheListener>>();

  get<T>(key: CacheKey): CacheEntry<T> | undefined {
    return this.#entries.get(key) as CacheEntry<T> | undefined;
  }

  set<T>(key: CacheKey, entry: CacheEntry<T>): void {
    this.#entries.set(key, entry as CacheEntry);
    this.#notify(key);
  }

  update<T>(key: CacheKey, fn: (data: T) => T): void {
    const prev = this.get<T>(key);
    if (!prev) return;
    this.set(key, {
      ...prev,
      data: fn(prev.data),
      updatedAt: Date.now()
    });
  }

  /** Delete a single key (exact). */
  delete(key: CacheKey): void {
    if (this.#entries.delete(key)) this.#notify(key);
  }

  invalidate(prefix?: string): void {
    if (prefix === undefined) {
      const keys = [...this.#entries.keys()];
      this.#entries.clear();
      for (const k of keys) this.#notify(k);
      return;
    }
    for (const key of [...this.#entries.keys()]) {
      if (key.startsWith(prefix)) {
        this.#entries.delete(key);
        this.#notify(key);
      }
    }
  }

  subscribe(key: CacheKey, cb: CacheListener): () => void {
    let set = this.#listeners.get(key);
    if (!set) {
      set = new Set();
      this.#listeners.set(key, set);
    }
    set.add(cb);
    return () => {
      set!.delete(cb);
      if (set!.size === 0) this.#listeners.delete(key);
    };
  }

  #notify(key: CacheKey): void {
    const set = this.#listeners.get(key);
    if (!set) return;
    for (const cb of set) cb();
  }
}

/** Stable key for a document string + variables. */
export function cacheKey(document: string, variables?: Record<string, unknown>): CacheKey {
  // Treat missing and empty variables as the same key.
  const vars =
    variables && Object.keys(variables).length > 0 ? stableStringify(variables) : '';
  return `${document.trim()}::${vars}`;
}

function stableStringify(value: unknown): string {
  if (value === null || typeof value !== 'object') {
    return JSON.stringify(value);
  }
  if (Array.isArray(value)) {
    return `[${value.map(stableStringify).join(',')}]`;
  }
  const obj = value as Record<string, unknown>;
  const keys = Object.keys(obj).sort();
  return `{${keys.map((k) => `${JSON.stringify(k)}:${stableStringify(obj[k])}`).join(',')}}`;
}
