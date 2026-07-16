/**
 * Command result pipeline:
 * optimistic → network → result.kind → effects → reconcile
 *
 * Product rule: command success ≠ projection visibility.
 * Default todos path: ack/fact + subscription/refetch — do not invent full RM rows from ack.
 */

import type { QueryCache } from './query-cache.ts';
import { cacheKey } from './query-cache.ts';
import {
  applyCacheOps,
  applyProjectionPayload,
  fx,
  rollback,
  type CacheOp,
  type CacheTarget,
  type CommandPolicy,
  type Effect,
  type ResultKind,
  type ReconcileKind
} from './ops.ts';

export type GqlError = { message?: string; extensions?: { code?: string } };

export type NetworkResult<T = Record<string, unknown>> = {
  data?: T | null;
  errors?: GqlError[] | null;
};

export type CommandPipelineOptions = {
  policy?: CommandPolicy;
  optimistic?: CommandPolicy['optimistic'];
  result?: CommandPolicy['result'];
  reconcile?: CommandPolicy['reconcile'];
  onSuccess?: (ctx: {
    data: unknown;
  }) => Array<CacheOp | Effect>;
  onError?: (ctx: { errors: GqlError[] }) => Array<CacheOp | Effect>;
  onSettled?: () => void;
  /** When true (SSR), skip optimistic + cache mutations. */
  browser?: boolean;
};

export type PipelineDeps = {
  cache: QueryCache;
  request: (document: string, variables?: Record<string, unknown>) => Promise<NetworkResult>;
  /** Optional: run refetch reconcile. */
  refetch?: (document: string, variables?: Record<string, unknown>) => Promise<NetworkResult>;
  /** Schedule effects (toast/alert). Never called on the network path itself. */
  runEffects?: (effects: Effect[]) => void;
};

function mergePolicy(
  defaults: CommandPolicy | undefined,
  opts: CommandPipelineOptions
): {
  resultKind: ResultKind;
  applyTargets?: CacheTarget[];
  reconcileKind: ReconcileKind;
  reconcileDoc?: string;
  reconcileVars?: Record<string, unknown>;
  optimistic?: CommandPolicy['optimistic'];
} {
  const result = opts.result ?? defaults?.result;
  const reconcile = opts.reconcile ?? defaults?.reconcile;
  return {
    resultKind: result?.kind ?? 'ack',
    applyTargets: result?.apply?.targets,
    reconcileKind: reconcile?.kind ?? 'none',
    reconcileDoc: reconcile?.document,
    reconcileVars: reconcile?.variables,
    optimistic: opts.optimistic ?? defaults?.optimistic
  };
}

function splitOps(items: Array<CacheOp | Effect>): { ops: CacheOp[]; effects: Effect[] } {
  const ops: CacheOp[] = [];
  const effects: Effect[] = [];
  for (const item of items) {
    if ('op' in item) ops.push(item);
    else effects.push(item);
  }
  return { ops, effects };
}

/**
 * Run one generated command mutation through the cache pipeline.
 * Exactly one network `request` for the command document.
 */
export async function runCommandPipeline<T = Record<string, unknown>>(
  deps: PipelineDeps,
  commandDocument: string,
  input: Record<string, unknown>,
  opts: CommandPipelineOptions = {}
): Promise<NetworkResult<T>> {
  const browser = opts.browser !== false;
  const merged = mergePolicy(opts.policy, opts);
  let snaps = [] as ReturnType<typeof applyCacheOps>;

  if (browser && merged.optimistic?.targets?.length && merged.optimistic.row) {
    const optOps: CacheOp[] = merged.optimistic.targets.map((target) => ({
      op: 'upsert' as const,
      target,
      row: { ...merged.optimistic!.row! }
    }));
    snaps = applyCacheOps(deps.cache, optOps);
  }

  let result: NetworkResult<T>;
  try {
    result = (await deps.request(commandDocument, { input })) as NetworkResult<T>;
  } catch (e) {
    if (browser && snaps.length) rollback(deps.cache, snaps);
    const errors = [{ message: e instanceof Error ? e.message : String(e) }];
    const errItems = opts.onError?.({ errors }) ?? [];
    const { ops, effects } = splitOps(errItems);
    if (browser && ops.length) applyCacheOps(deps.cache, ops);
    deps.runEffects?.(effects);
    opts.onSettled?.();
    return { data: null, errors };
  }

  if (result.errors?.length) {
    if (browser && snaps.length) rollback(deps.cache, snaps);
    const errItems = opts.onError?.({ errors: result.errors }) ?? [
      fx.alert(result.errors[0]?.message ?? 'Failed')
    ];
    const { ops, effects } = splitOps(errItems);
    if (browser && ops.length) applyCacheOps(deps.cache, ops);
    deps.runEffects?.(effects);
    opts.onSettled?.();
    return result;
  }

  // Success path
  if (browser) {
    if (merged.resultKind === 'projection') {
      const data = result.data;
      let payload: Record<string, unknown> | null = null;
      if (data && typeof data === 'object' && !Array.isArray(data)) {
        const values = Object.values(data as Record<string, unknown>);
        const first = values[0];
        if (first && typeof first === 'object' && !Array.isArray(first)) {
          payload = first as Record<string, unknown>;
        } else {
          payload = data as Record<string, unknown>;
        }
      }
      if (payload && merged.applyTargets?.length) {
        // Payload fields only — applyProjectionPayload copies payload keys only.
        applyProjectionPayload(deps.cache, merged.applyTargets, payload);
      }
    } else if (merged.resultKind === 'ack' || merged.resultKind === 'fact') {
      // Do NOT invent a full list row from incomplete mutation payload.
      // Keep optimistic (if any) and mark pending when we have a cache key.
      if (merged.optimistic?.targets?.length) {
        for (const t of merged.optimistic.targets) {
          const key = cacheKey(t.document, t.variables);
          const entry = deps.cache.get(key);
          if (entry) {
            deps.cache.set(key, { ...entry, pending: true, optimistic: true });
          }
        }
      }
    }
    // resultKind === 'none' → no cache apply from response
  }

  const successItems = opts.onSuccess?.({ data: result.data }) ?? [];
  const { ops, effects } = splitOps(successItems);
  if (browser && ops.length) applyCacheOps(deps.cache, ops);
  deps.runEffects?.(effects);

  // Reconcile (subscription is usually already running; refetch optional)
  if (browser && merged.reconcileKind === 'refetch' && merged.reconcileDoc && deps.refetch) {
    const refetched = await deps.refetch(merged.reconcileDoc, merged.reconcileVars);
    if (refetched.data && !refetched.errors?.length) {
      const key = cacheKey(merged.reconcileDoc, merged.reconcileVars);
      deps.cache.set(key, {
        data: refetched.data,
        updatedAt: Date.now(),
        pending: false,
        optimistic: false
      });
    }
  } else if (browser && merged.reconcileKind === 'invalidate' && merged.reconcileDoc) {
    deps.cache.invalidate(cacheKey(merged.reconcileDoc, merged.reconcileVars));
  }
  // subscription: caller owns live query → cache write-through separately

  opts.onSettled?.();
  return result;
}

export { fx };
export type { CacheOp, Effect, CommandPolicy, ResultKind, ReconcileKind };
