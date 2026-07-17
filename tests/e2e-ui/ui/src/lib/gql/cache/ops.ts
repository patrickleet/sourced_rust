/**
 * Closed sets: CacheOp (client cache only) vs Effect (UI side effects).
 */

import type { QueryCache } from './query-cache.ts';
import { cacheKey } from './query-cache.ts';

export type CacheTarget = {
  /** Query/subscription document string. */
  document: string;
  variables?: Record<string, unknown>;
  /** Dot path to list/object on data, e.g. "todos". Empty = whole document. */
  at?: string;
  /** Primary key field for list upsert/remove. */
  by?: string;
};

export type CacheOp =
  | {
      op: 'upsert';
      target: CacheTarget;
      row: Record<string, unknown>;
    }
  | {
      op: 'patch';
      target: CacheTarget;
      row: Record<string, unknown>;
    }
  | {
      op: 'remove';
      target: CacheTarget;
      id: unknown;
    }
  | {
      op: 'write';
      target: CacheTarget;
      data: unknown;
    }
  | {
      op: 'invalidate';
      prefix?: string;
    };

export type Effect =
  | { kind: 'toast'; message: string }
  | { kind: 'alert'; message: string }
  | { kind: 'goto'; path: string };

/**
 * UI side-effect constructors for the command pipeline.
 * Named `fx` (not `effect`) so pages never shadow Svelte 5's `$effect` rune.
 */
export const fx = {
  toast: (message: string): Effect => ({ kind: 'toast', message }),
  alert: (message: string): Effect => ({ kind: 'alert', message }),
  goto: (path: string): Effect => ({ kind: 'goto', path })
};

export type ResultKind = 'ack' | 'fact' | 'projection' | 'none';
export type ReconcileKind = 'subscription' | 'refetch' | 'invalidate' | 'none';

/** How to merge server list data under pending optimistic rows (required for list docs). */
export type ListMergeSpec = {
  /** Dot path to the list on the document, e.g. `"todos"`. */
  at: string;
  /** Primary key field on each row, e.g. `"todo_id"`. */
  by: string;
};

export type CommandPolicy = {
  result?: { kind: ResultKind; apply?: { targets: CacheTarget[] } };
  reconcile?: {
    kind: ReconcileKind;
    document?: string;
    variables?: Record<string, unknown>;
    /** Required when reconciling list documents with pending optimistic rows. */
    list?: ListMergeSpec;
  };
  optimistic?: {
    targets: CacheTarget[];
    row?: Record<string, unknown>;
  };
};

export type Snapshot = {
  key: string;
  entry: ReturnType<QueryCache['get']> | undefined;
  had: boolean;
};

function getAt(data: unknown, path?: string): unknown {
  if (!path) return data;
  let cur: unknown = data;
  for (const part of path.split('.').filter(Boolean)) {
    if (cur === null || typeof cur !== 'object') return undefined;
    cur = (cur as Record<string, unknown>)[part];
  }
  return cur;
}

function setAt(data: unknown, path: string | undefined, value: unknown): unknown {
  if (!path) return value;
  const parts = path.split('.').filter(Boolean);
  const root =
    data !== null && typeof data === 'object' ? { ...(data as Record<string, unknown>) } : {};
  let cur: Record<string, unknown> = root;
  for (let i = 0; i < parts.length - 1; i++) {
    const p = parts[i]!;
    const next = cur[p];
    cur[p] =
      next !== null && typeof next === 'object' && !Array.isArray(next)
        ? { ...(next as Record<string, unknown>) }
        : {};
    cur = cur[p] as Record<string, unknown>;
  }
  cur[parts[parts.length - 1]!] = value;
  return root;
}

function cloneEntry(entry: ReturnType<QueryCache['get']>): ReturnType<QueryCache['get']> {
  if (!entry) return undefined;
  return {
    ...entry,
    data: structuredClone(entry.data)
  };
}

export function applyCacheOp(cache: QueryCache, op: CacheOp, now = Date.now()): Snapshot | null {
  if (op.op === 'invalidate') {
    // Exact key only (full cacheKey). Use QueryCache.invalidatePrefix for intentional prefix wipe.
    cache.invalidate(op.prefix);
    return null;
  }

  const key = cacheKey(op.target.document, op.target.variables);
  const prev = cache.get(key);
  const snap: Snapshot = { key, entry: cloneEntry(prev), had: !!prev };

  if (op.op === 'write') {
    cache.set(key, {
      data: op.data,
      updatedAt: now,
      optimistic: false,
      pending: false
    });
    return snap;
  }

  const baseData = prev?.data ?? {};
  const at = op.target.at;
  const by = op.target.by ?? 'id';

  if (op.op === 'upsert' || op.op === 'patch') {
    const list = getAt(baseData, at);
    if (Array.isArray(list)) {
      const id = op.row[by];
      const idx = list.findIndex(
        (row) => row && typeof row === 'object' && (row as Record<string, unknown>)[by] === id
      );
      const next = [...list];
      if (idx >= 0) {
        const existing = next[idx] as Record<string, unknown>;
        next[idx] = op.op === 'patch' ? { ...existing, ...op.row } : { ...existing, ...op.row };
      } else if (op.op === 'upsert') {
        next.push({ ...op.row });
      }
      const data = setAt(baseData, at, next);
      cache.set(key, {
        data,
        updatedAt: now,
        optimistic: true,
        pending: prev?.pending
      });
    } else if (!prev) {
      // No cached list yet — only write full document if `at` empty.
      if (!at) {
        cache.set(key, {
          data: op.row,
          updatedAt: now,
          optimistic: true
        });
      }
      // else: do not invent a list document from incomplete row
    } else if (!at && op.op === 'patch' && prev.data && typeof prev.data === 'object') {
      cache.set(key, {
        data: { ...(prev.data as object), ...op.row },
        updatedAt: now,
        optimistic: true
      });
    }
    return snap;
  }

  if (op.op === 'remove') {
    const list = getAt(baseData, at);
    if (Array.isArray(list)) {
      const next = list.filter(
        (row) => !(row && typeof row === 'object' && (row as Record<string, unknown>)[by] === op.id)
      );
      cache.set(key, {
        data: setAt(baseData, at, next),
        updatedAt: now,
        optimistic: false
      });
    }
    return snap;
  }

  return snap;
}

/** Apply ops; return snapshots for rollback (in reverse order). */
export function applyCacheOps(cache: QueryCache, ops: CacheOp[]): Snapshot[] {
  const snaps: Snapshot[] = [];
  for (const op of ops) {
    const snap = applyCacheOp(cache, op);
    if (snap) snaps.push(snap);
  }
  return snaps;
}

export function rollback(cache: QueryCache, snaps: Snapshot[]): void {
  // Restore in reverse so nested writes unwind correctly.
  for (let i = snaps.length - 1; i >= 0; i--) {
    const s = snaps[i]!;
    if (s.had && s.entry) {
      cache.set(s.key, s.entry);
    } else {
      cache.delete(s.key);
    }
  }
}

/**
 * Projection apply: merge payload fields only onto cache targets.
 * Never invents fields absent from `payload`.
 */
export function applyProjectionPayload(
  cache: QueryCache,
  targets: CacheTarget[],
  payload: Record<string, unknown>
): Snapshot[] {
  const ops: CacheOp[] = targets.map((target) => ({
    op: 'upsert' as const,
    target,
    row: { ...payload }
  }));
  return applyCacheOps(cache, ops);
}

export type WriteServerOptions = {
  /**
   * List merge for pending/optimistic entries.
   * **Required** for list documents under pending — no silent `todos`/`todo_id` guessing.
   * When omitted while pending, local cache is left unchanged (safe).
   */
  list?: ListMergeSpec;
};

/**
 * Write a server query/subscription payload into the cache **without** clobbering
 * optimistic rows that the projector has not caught up to yet.
 *
 * Fixes archive/complete flicker: optimistic → immediate refetch (stale RM) → late refetch.
 * While `pending`/`optimistic`, keep local rows that still differ from server by PK.
 */
export function writeServerDataPreservingPending(
  cache: QueryCache,
  document: string,
  variables: Record<string, unknown> | undefined,
  serverData: unknown,
  options?: WriteServerOptions
): void {
  const key = cacheKey(document, variables);
  const prev = cache.get(key);
  const now = Date.now();

  if (!prev?.pending && !prev?.optimistic) {
    cache.set(key, {
      data: serverData,
      updatedAt: now,
      pending: false,
      optimistic: false
    });
    return;
  }

  const prevData = prev.data;
  if (
    !prevData ||
    typeof prevData !== 'object' ||
    !serverData ||
    typeof serverData !== 'object'
  ) {
    // Can't merge — keep optimistic until a later refetch
    return;
  }

  const list = options?.list;
  if (!list?.at || !list?.by) {
    // No explicit list merge — never invent path/PK; keep pending local data.
    return;
  }

  const path = list.at;
  const by = list.by;

  const serverList = getAt(serverData, path);
  const localList = getAt(prevData, path);
  if (!Array.isArray(serverList) || !Array.isArray(localList)) {
    return;
  }

  const serverById = new Map<unknown, Record<string, unknown>>();
  for (const row of serverList) {
    if (row && typeof row === 'object') {
      const r = row as Record<string, unknown>;
      serverById.set(r[by], r);
    }
  }

  const localById = new Map<unknown, Record<string, unknown>>();
  for (const row of localList) {
    if (row && typeof row === 'object') {
      const r = row as Record<string, unknown>;
      localById.set(r[by], r);
    }
  }

  const merged: Record<string, unknown>[] = [];
  const seen = new Set<unknown>();

  // Prefer server order; overlay optimistic when server still lags
  for (const sRow of serverList) {
    if (!sRow || typeof sRow !== 'object') continue;
    const s = sRow as Record<string, unknown>;
    const id = s[by];
    seen.add(id);
    const local = localById.get(id);
    if (local && shallowRowDiffers(local, s)) {
      merged.push({ ...local });
    } else {
      merged.push({ ...s });
    }
  }

  // Optimistic creates not yet on server
  for (const [id, local] of localById) {
    if (!seen.has(id) && !serverById.has(id)) {
      merged.push({ ...local });
    }
  }

  let stillPending = false;
  for (const row of merged) {
    const id = row[by];
    const s = serverById.get(id);
    if (!s || shallowRowDiffers(row, s)) {
      stillPending = true;
      break;
    }
  }

  const data = setAt(serverData, path, merged);
  cache.set(key, {
    data,
    updatedAt: now,
    pending: stillPending,
    optimistic: stillPending
  });
}

/** True if any shared key differs (status, title, …). */
function shallowRowDiffers(a: Record<string, unknown>, b: Record<string, unknown>): boolean {
  const keys = new Set([...Object.keys(a), ...Object.keys(b)]);
  for (const k of keys) {
    if (a[k] !== b[k]) return true;
  }
  return false;
}
