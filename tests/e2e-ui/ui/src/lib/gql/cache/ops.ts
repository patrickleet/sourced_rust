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

export const effect = {
  toast: (message: string): Effect => ({ kind: 'toast', message }),
  alert: (message: string): Effect => ({ kind: 'alert', message }),
  goto: (path: string): Effect => ({ kind: 'goto', path })
};

export type ResultKind = 'ack' | 'fact' | 'projection' | 'none';
export type ReconcileKind = 'subscription' | 'refetch' | 'invalidate' | 'none';

export type CommandPolicy = {
  result?: { kind: ResultKind; apply?: { targets: CacheTarget[] } };
  reconcile?: { kind: ReconcileKind; document?: string; variables?: Record<string, unknown> };
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
