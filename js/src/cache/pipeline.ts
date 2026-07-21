/** Optimistic -> network -> result -> effects -> reconcile command pipeline. */
import type { GqlError, GraphqlVariables } from '../types.js';
import {
	applyCacheOps,
	applyProjectionPayload,
	fx,
	rollback,
	writeServerDataPreservingPending,
	type CacheOp,
	type CacheTarget,
	type CommandPolicy,
	type Effect,
	type ReconcileKind,
	type ResultKind
} from './ops.js';
import { cacheKey, type QueryCache } from './query-cache.js';

export type NetworkResult<TData = Record<string, unknown>> = {
	data?: TData | null;
	errors?: GqlError[] | null;
	/** HTTP status when the transport has one. Network exceptions use 0. */
	status?: number;
};

export type CommandPipelineOptions = {
	policy?: CommandPolicy;
	optimistic?: CommandPolicy['optimistic'];
	result?: CommandPolicy['result'];
	reconcile?: CommandPolicy['reconcile'];
	onSuccess?: (context: { data: unknown }) => Array<CacheOp | Effect>;
	onError?: (context: { errors: GqlError[] }) => Array<CacheOp | Effect>;
	onSettled?: () => void;
	/** Set false during SSR to skip optimistic and cache mutations. */
	browser?: boolean;
};

export type PipelineDeps = {
	cache: QueryCache;
	request: (
		document: string,
		variables?: GraphqlVariables
	) => Promise<NetworkResult>;
	refetch?: (
		document: string,
		variables?: GraphqlVariables
	) => Promise<NetworkResult>;
	runEffects?: (effects: Effect[]) => void;
};

type MergedPolicy = {
	resultKind: ResultKind;
	applyTargets?: CacheTarget[];
	reconcileKind: ReconcileKind;
	reconcileDocument?: string;
	reconcileVariables?: GraphqlVariables;
	optimistic?: CommandPolicy['optimistic'];
};

function mergePolicy(
	defaults: CommandPolicy | undefined,
	options: CommandPipelineOptions
): MergedPolicy {
	const result = options.result ?? defaults?.result;
	const reconcile = options.reconcile ?? defaults?.reconcile;
	return {
		resultKind: result?.kind ?? 'ack',
		applyTargets: result?.apply?.targets,
		reconcileKind: reconcile?.kind ?? 'none',
		reconcileDocument: reconcile?.document,
		reconcileVariables: reconcile?.variables,
		optimistic: options.optimistic ?? defaults?.optimistic
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

/** Execute one generated command through the configured cache pipeline. */
export async function runCommandPipeline<TData = Record<string, unknown>>(
	deps: PipelineDeps,
	commandDocument: string,
	input: Record<string, unknown> | undefined,
	options: CommandPipelineOptions = {}
): Promise<NetworkResult<TData>> {
	const browser = options.browser !== false;
	const policy = mergePolicy(options.policy, options);
	const cacheGeneration = deps.cache.generation;
	const cacheIsCurrent = () => deps.cache.generation === cacheGeneration;
	let snapshots: ReturnType<typeof applyCacheOps> = [];

	if (browser && policy.optimistic?.targets.length && policy.optimistic.row) {
		snapshots = applyCacheOps(
			deps.cache,
			policy.optimistic.targets.map((target) => ({
				op: 'upsert',
				target,
				row: { ...policy.optimistic!.row! }
			}))
		);
	}

	let result: NetworkResult<TData>;
	try {
		result = (await deps.request(
			commandDocument,
			input === undefined ? undefined : { input }
		)) as NetworkResult<TData>;
	} catch (error) {
		const errors: GqlError[] = [
			{ message: error instanceof Error ? error.message : String(error) }
		];
		if (!cacheIsCurrent()) {
			options.onSettled?.();
			return { data: null, errors, status: 0 };
		}
		if (browser && snapshots.length) rollback(deps.cache, snapshots);
		const { ops, effects } = splitOps(options.onError?.({ errors }) ?? []);
		if (browser && ops.length) applyCacheOps(deps.cache, ops);
		deps.runEffects?.(effects);
		options.onSettled?.();
		return { data: null, errors, status: 0 };
	}

	if (!cacheIsCurrent()) {
		options.onSettled?.();
		return result;
	}

	if (result.errors?.length) {
		if (browser && snapshots.length) rollback(deps.cache, snapshots);
		const { ops, effects } = splitOps(
			options.onError?.({ errors: result.errors }) ?? [
				fx.alert(result.errors[0]?.message ?? 'Failed')
			]
		);
		if (browser && ops.length) applyCacheOps(deps.cache, ops);
		deps.runEffects?.(effects);
		options.onSettled?.();
		return result;
	}

	if (browser) {
		if (policy.resultKind === 'projection') {
			const payload = projectionPayload(result.data);
			if (payload && policy.applyTargets?.length) {
				applyProjectionPayload(deps.cache, policy.applyTargets, payload);
			}
		} else if (
			(policy.resultKind === 'ack' || policy.resultKind === 'fact') &&
			policy.optimistic?.targets.length
		) {
			for (const target of policy.optimistic.targets) {
				const key = cacheKey(target.document, target.variables);
				const entry = deps.cache.get(key);
				if (entry) deps.cache.set(key, { ...entry, pending: true, optimistic: true });
			}
		}
	}

	const { ops, effects } = splitOps(options.onSuccess?.({ data: result.data }) ?? []);
	if (browser && ops.length) applyCacheOps(deps.cache, ops);
	deps.runEffects?.(effects);

	if (
		browser &&
		cacheIsCurrent() &&
		policy.reconcileKind === 'refetch' &&
		policy.reconcileDocument &&
		deps.refetch
	) {
		const refetched = await deps.refetch(
			policy.reconcileDocument,
			policy.reconcileVariables
		);
		if (
			cacheIsCurrent() &&
			refetched.data !== undefined &&
			refetched.data !== null &&
			!refetched.errors?.length
		) {
			const reconcileList = (options.reconcile ?? options.policy?.reconcile)?.list;
			writeServerDataPreservingPending(
				deps.cache,
				policy.reconcileDocument,
				policy.reconcileVariables,
				refetched.data,
				reconcileList ? { list: reconcileList } : undefined
			);
		}
	} else if (
		browser &&
		cacheIsCurrent() &&
		policy.reconcileKind === 'invalidate' &&
		policy.reconcileDocument
	) {
		deps.cache.invalidate(
			cacheKey(policy.reconcileDocument, policy.reconcileVariables)
		);
	}

	options.onSettled?.();
	return result;
}

function projectionPayload(data: unknown): Record<string, unknown> | null {
	if (data === null || typeof data !== 'object' || Array.isArray(data)) return null;
	const record = data as Record<string, unknown>;
	const first = Object.values(record)[0];
	return first !== null && typeof first === 'object' && !Array.isArray(first)
		? (first as Record<string, unknown>)
		: record;
}

export { fx };
export type {
	CacheOp,
	CommandPolicy,
	Effect,
	ReconcileKind,
	ResultKind
};
