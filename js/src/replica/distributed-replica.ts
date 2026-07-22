import {
	CacheRevisionConflictError,
	createCacheEngine,
	type BaseCacheWriter,
	type CacheEngine,
	type CacheIndexCoverage,
	type CacheIndexMetadata,
	type OptimisticCacheWriter
} from '../internal/cache-engine.js';
import type { GqlError, GraphqlVariables } from '../types.js';
import {
	canonicalVariables,
	cloneJsonObject,
	cloneJsonValue,
	replicaIndexKey,
	replicaRecordKey,
	resolveArguments
} from './identity.js';
import {
	materializeReplicaOperation,
	type MaterializedReplicaResult
} from './materialize.js';
import { normalizeReplicaResult } from './normalize.js';
import type {
	DistributedReplicaOptions,
	DistributedReplica as DistributedReplicaApi,
	ReplicaBaseWriter,
	ReplicaIdentity,
	ReplicaIndexInspection,
	ReplicaIndexTarget,
	ReplicaLiveState,
	ReplicaModelArtifact,
	ReplicaOperationArtifact,
	ReplicaOptimisticWriter,
	ReplicaRecordInspection,
	ReplicaRecordPatch,
	ReplicaRevision,
	ReplicaResultEnvelope,
	ReplicaSnapshot,
	ReplicaStatus,
	ReplicaTransport,
	ReplicaWatch,
	ReplicaWriteSource,
	WatchReplicaOptions
} from './types.js';

type QueryState = {
	fetching: boolean;
	errors: readonly GqlError[];
	live: ReplicaLiveState;
	latestRevision?: bigint;
};

type LiveEntry = {
	count: number;
	unsubscribe: () => void;
	active: boolean;
};

const EMPTY_ERRORS: readonly GqlError[] = Object.freeze([]);

class DistributedReplicaImpl implements DistributedReplicaApi {
	readonly #engine: CacheEngine;
	readonly #transport: ReplicaTransport | undefined;
	readonly #reportObserverError: (error: AggregateError) => void;
	readonly #inFlight = new Map<string, Promise<void>>();
	readonly #queryStates = new Map<string, QueryState>();
	readonly #watches = new Map<string, Set<ReplicaWatchState<unknown, GraphqlVariables>>>();
	readonly #lives = new Map<string, LiveEntry>();

	constructor(options: DistributedReplicaOptions = {}) {
		this.#transport = options.transport;
		this.#reportObserverError = options.onObserverError ?? reportUnhandledObserverError;
		this.#engine = createCacheEngine({ onWatcherError: this.#reportObserverError });
	}

	read<TData, TVariables extends GraphqlVariables>(
		artifact: ReplicaOperationArtifact<TData, TVariables>,
		variables: TVariables
	): ReplicaSnapshot<TData> {
		const stableVariables = cloneJsonObject(variables) as TVariables;
		const key = operationKey(artifact, stableVariables);
		const materialized = this.#engine.read((reader) =>
			materializeReplicaOperation(reader, artifact, stableVariables)
		);
		return snapshotFrom(materialized, this.#queryState(key));
	}

	watch<TData, TVariables extends GraphqlVariables>(
		artifact: ReplicaOperationArtifact<TData, TVariables>,
		variables: TVariables,
		options: WatchReplicaOptions = {}
	): ReplicaWatch<TData> {
		return new ReplicaWatchState(this, artifact, variables, options);
	}

	writeResult<TData, TVariables extends GraphqlVariables>(
		artifact: ReplicaOperationArtifact<TData, TVariables>,
		variables: TVariables,
		envelope: ReplicaResultEnvelope<TData>,
		source: ReplicaWriteSource
	): void {
		assertWriteSource(source);
		const stableVariables = cloneJsonObject(variables) as TVariables;
		const key = operationKey(artifact, stableVariables);
		const state = this.#queryState(key);
		const incomingRevision = revisionToken(envelope.revision);
		if (state.latestRevision !== undefined && incomingRevision < state.latestRevision) {
			return;
		}
		const previousErrors = state.errors;
		state.errors = stableErrors(previousErrors, envelope.errors ?? []);
		try {
			this.#engine.batch((writer) => {
				const summary = normalizeReplicaResult(
					writer,
					artifact,
					stableVariables,
					envelope
				);
				if ((envelope.errors?.length ?? 0) > 0 && summary.indexKeys.length === 0) {
					for (const root of artifact.roots) {
						const argumentsValue = resolveArguments(root.arguments, stableVariables);
						writer.markIndexStale(
							replicaIndexKey({ field: root.field, arguments: argumentsValue }),
							'graphql-error',
							envelope.revision
						);
					}
				}
			});
		} catch (error) {
			state.errors = previousErrors;
			if (error instanceof CacheRevisionConflictError) {
				this.#markArtifactIndexesStale(
					artifact,
					stableVariables,
					'revision-conflict',
					envelope.revision
				);
			}
			this.#emitState(key, false);
			throw error;
		}
		state.latestRevision = incomingRevision;
		this.#emitState(key, false);
	}

	createOptimisticLayer(
		id: string,
		update: (writer: ReplicaOptimisticWriter) => void
	): void {
		this.#engine.createOptimisticLayer(id, (writer) => update(optimisticWriter(writer)));
	}

	markOptimisticLayerAccepted(id: string): boolean {
		return this.#engine.markOptimisticLayerAccepted(id);
	}

	confirmOptimisticLayer<T>(
		id: string,
		update: (writer: ReplicaBaseWriter) => T
	): T {
		return this.#engine.confirmOptimisticLayer(id, (writer) => update(baseWriter(writer)));
	}

	rejectOptimisticLayer(id: string): boolean {
		return this.#engine.rejectOptimisticLayer(id);
	}

	tombstoneRecord(
		model: ReplicaModelArtifact,
		identity: ReplicaIdentity,
		revision: ReplicaRevision
	): boolean {
		return this.#engine.batch((writer) =>
			writer.tombstoneRecord(replicaRecordKey(model, identity), revision)
		);
	}

	markIndexStale(target: ReplicaIndexTarget, reason: string): boolean {
		return this.#engine.batch((writer) =>
			writer.markIndexStale(indexKeyFromTarget(target), reason)
		);
	}

	retainRecord(model: ReplicaModelArtifact, identity: ReplicaIdentity): void {
		this.#engine.retain(replicaRecordKey(model, identity));
	}

	releaseRecord(model: ReplicaModelArtifact, identity: ReplicaIdentity): void {
		this.#engine.release(replicaRecordKey(model, identity));
	}

	gc(): readonly string[] {
		return this.#engine.gc();
	}

	inspectRecord(
		model: ReplicaModelArtifact,
		identity: ReplicaIdentity
	): ReplicaRecordInspection | undefined {
		const key = replicaRecordKey(model, identity);
		return this.#engine.read((reader) => {
			const record = reader.record(key);
			if (!record) return undefined;
			return Object.freeze({
				key,
				revision: record.revision,
				incarnation: record.incarnation,
				presentFields: Object.freeze(Object.keys(record.fields).sort())
			});
		});
	}

	inspectIndex(target: ReplicaIndexTarget): ReplicaIndexInspection | undefined {
		const key = indexKeyFromTarget(target);
		return this.#engine.read((reader) => {
			const index = reader.index(key);
			if (!index?.metadata) return undefined;
			return Object.freeze({
				key,
				revision: index.revision,
				...(index.staleRevision === undefined
					? {}
					: { staleRevision: index.staleRevision }),
				records: Object.freeze([...index.records]),
				complete: index.complete,
				field: index.metadata.field,
				...(index.metadata.parent === undefined
					? {}
					: { parent: index.metadata.parent }),
				arguments: index.metadata.arguments,
				coverage: index.metadata.coverage,
				dependencies: index.metadata.dependencies,
				...(index.metadata.staleReason === undefined
					? {}
					: { staleReason: index.metadata.staleReason }),
				nullValue: index.metadata.nullValue === true
			});
		});
	}

	/** Package-internal hook used by one watched operation. */
	_register<TData, TVariables extends GraphqlVariables>(
		watch: ReplicaWatchState<TData, TVariables>
	): () => void {
		let watches = this.#watches.get(watch.key);
		if (!watches) {
			watches = new Set();
			this.#watches.set(
				watch.key,
				watches as Set<ReplicaWatchState<unknown, GraphqlVariables>>
			);
		}
		watches.add(watch as ReplicaWatchState<unknown, GraphqlVariables>);
		const unwatch = this.#engine.watch(
			(reader) => materializeReplicaOperation(reader, watch.artifact, watch.variables),
			(materialized) => watch._cacheChanged(materialized)
		);
		if (watch.liveRequested) this.#retainLive(watch);
		void this._fetch(watch, false);
		return () => {
			unwatch();
			watches?.delete(watch as ReplicaWatchState<unknown, GraphqlVariables>);
			if (watches?.size === 0) this.#watches.delete(watch.key);
			if (watch.liveRequested) this.#releaseLive(watch.key);
		};
	}

	/** Package-internal query-state lookup. */
	_state(key: string): QueryState {
		return this.#queryState(key);
	}

	/** Package-internal materialization for watch construction. */
	_materialize<TData, TVariables extends GraphqlVariables>(
		artifact: ReplicaOperationArtifact<TData, TVariables>,
		variables: TVariables
	): MaterializedReplicaResult<TData> {
		return this.#engine.read((reader) =>
			materializeReplicaOperation(reader, artifact, variables)
		);
	}

	/** Package-internal cache-and-live coordinator. */
	_fetch<TData, TVariables extends GraphqlVariables>(
		watch: ReplicaWatchState<TData, TVariables>,
		force: boolean
	): Promise<void> {
		if (!this.#transport) return Promise.resolve();
		if (!force && watch.materialized.complete && !watch.materialized.stale) {
			return Promise.resolve();
		}
		const existing = this.#inFlight.get(watch.key);
		if (existing) return existing;

		const state = this.#queryState(watch.key);
		state.fetching = true;
		this.#emitState(watch.key, false);
		const request = Object.freeze({
			operation: 'query' as const,
			operationId: watch.artifact.id,
			document: watch.artifact.document,
			variables: watch.variables,
			artifact: watch.artifact
		});
		let flight: Promise<void>;
		flight = Promise.resolve()
			.then(() => this.#transport!.fetch(request))
			.then((result) => {
				if (this.#inFlight.get(watch.key) !== flight) return;
				this.writeResult(watch.artifact, watch.variables, result, 'network');
			})
			.catch((error: unknown) => {
				if (this.#inFlight.get(watch.key) !== flight) return;
				state.errors = stableErrors(state.errors, [graphqlError(error)]);
			})
			.finally(() => {
				if (this.#inFlight.get(watch.key) !== flight) return;
				this.#inFlight.delete(watch.key);
				state.fetching = false;
				this.#emitState(watch.key, false);
			});
		this.#inFlight.set(watch.key, flight);
		return flight;
	}

	_reportObserverErrors(errors: unknown[]): void {
		if (errors.length === 0) return;
		reportSafely(
			this.#reportObserverError,
			new AggregateError(errors, 'replica observer delivery failed')
		);
	}

	#queryState(key: string): QueryState {
		let state = this.#queryStates.get(key);
		if (!state) {
			state = { fetching: false, errors: EMPTY_ERRORS, live: 'off' };
			this.#queryStates.set(key, state);
		}
		return state;
	}

	#emitState(key: string, allowFetch: boolean): void {
		for (const watch of this.#watches.get(key) ?? []) watch._stateChanged(allowFetch);
	}

	#retainLive<TData, TVariables extends GraphqlVariables>(
		watch: ReplicaWatchState<TData, TVariables>
	): void {
		if (!watch.artifact.live || !this.#transport?.subscribe) return;
		const existing = this.#lives.get(watch.key);
		if (existing) {
			existing.count += 1;
			return;
		}
		const state = this.#queryState(watch.key);
		state.live = 'connecting';
		const entry: LiveEntry = { count: 1, unsubscribe: () => undefined, active: true };
		this.#lives.set(watch.key, entry);
		try {
			const unsubscribe = this.#transport.subscribe(
				Object.freeze({
					operation: 'live' as const,
					operationId: watch.artifact.live.id,
					document: watch.artifact.live.document,
					variables: watch.variables,
					artifact: watch.artifact
				}),
				{
					next: (result) => {
						if (!entry.active || this.#lives.get(watch.key) !== entry) return;
						state.live = 'active';
						try {
							this.writeResult(watch.artifact, watch.variables, result, 'live');
						} catch (error) {
							state.live = 'error';
							state.errors = stableErrors(state.errors, [graphqlError(error)]);
							this.#emitState(watch.key, false);
						}
					},
					error: (error) => {
						if (!entry.active || this.#lives.get(watch.key) !== entry) return;
						entry.active = false;
						this.#lives.delete(watch.key);
						state.live = 'error';
						state.errors = stableErrors(state.errors, [graphqlError(error)]);
						this.#emitState(watch.key, false);
					}
				}
			);
			entry.unsubscribe = unsubscribe;
			if (!entry.active || this.#lives.get(watch.key) !== entry) {
				unsubscribe();
				return;
			}
			state.live = 'active';
		} catch (error) {
			entry.active = false;
			this.#lives.delete(watch.key);
			state.live = 'error';
			state.errors = stableErrors(state.errors, [graphqlError(error)]);
		}
		this.#emitState(watch.key, false);
	}

	#releaseLive(key: string): void {
		const entry = this.#lives.get(key);
		if (!entry) return;
		entry.count -= 1;
		if (entry.count > 0) return;
		entry.active = false;
		this.#lives.delete(key);
		entry.unsubscribe();
		this.#queryState(key).live = 'off';
		this.#emitState(key, false);
	}

	#markArtifactIndexesStale<TData, TVariables extends GraphqlVariables>(
		artifact: ReplicaOperationArtifact<TData, TVariables>,
		variables: TVariables,
		reason: string,
		revision?: ReplicaRevision
	): void {
		this.#engine.batch((writer) => {
			for (const root of artifact.roots) {
				const argumentsValue = resolveArguments(root.arguments, variables);
				writer.markIndexStale(
					replicaIndexKey({ field: root.field, arguments: argumentsValue }),
					reason,
					revision
				);
			}
		});
	}
}

class ReplicaWatchState<TData, TVariables extends GraphqlVariables>
	implements ReplicaWatch<TData>
{
	readonly key: string;
	readonly artifact: ReplicaOperationArtifact<TData, TVariables>;
	readonly variables: TVariables;
	readonly liveRequested: boolean;
	materialized: MaterializedReplicaResult<TData>;
	readonly #owner: DistributedReplicaImpl;
	readonly #listeners = new Set<(snapshot: ReplicaSnapshot<TData>) => void>();
	#snapshot: ReplicaSnapshot<TData>;
	#identitySignature: string;
	#destroyed = false;
	readonly #unregister: () => void;

	constructor(
		owner: DistributedReplicaImpl,
		artifact: ReplicaOperationArtifact<TData, TVariables>,
		variables: TVariables,
		options: WatchReplicaOptions
	) {
		this.#owner = owner;
		this.artifact = artifact;
		this.variables = cloneJsonObject(variables) as TVariables;
		this.key = operationKey(artifact, this.variables);
		this.liveRequested = options.live === true;
		this.materialized = owner._materialize(artifact, this.variables);
		this.#identitySignature = this.materialized.identitySignature;
		this.#snapshot = snapshotFrom(this.materialized, owner._state(this.key));
		this.#unregister = owner._register(this);
	}

	get(): ReplicaSnapshot<TData> {
		return this.#snapshot;
	}

	subscribe(listener: (snapshot: ReplicaSnapshot<TData>) => void): () => void {
		if (this.#destroyed) throw new Error('replica watch is destroyed');
		if (typeof listener !== 'function') throw new TypeError('replica listener must be a function');
		this.#listeners.add(listener);
		try {
			listener(this.#snapshot);
		} catch (error) {
			this.#listeners.delete(listener);
			this.#owner._reportObserverErrors([error]);
		}
		return () => this.#listeners.delete(listener);
	}

	refresh(): Promise<void> {
		if (this.#destroyed) return Promise.reject(new Error('replica watch is destroyed'));
		return this.#owner._fetch(this, true);
	}

	destroy(): void {
		if (this.#destroyed) return;
		this.#destroyed = true;
		this.#unregister();
		this.#listeners.clear();
	}

	_cacheChanged(materialized: MaterializedReplicaResult<TData>): void {
		if (this.#destroyed) return;
		this.materialized = materialized;
		this.#sync(true);
	}

	_stateChanged(allowFetch: boolean): void {
		if (this.#destroyed) return;
		this.#sync(allowFetch);
	}

	#sync(allowFetch: boolean): void {
		const state = this.#owner._state(this.key);
		const nextRaw = snapshotFrom(this.materialized, state);
		const keepData =
			this.#identitySignature === this.materialized.identitySignature &&
			deepEqual(this.#snapshot.data, nextRaw.data);
		this.#identitySignature = this.materialized.identitySignature;
		const next: ReplicaSnapshot<TData> = keepData
			? (Object.freeze({
					...nextRaw,
					data: this.#snapshot.data
				}) as ReplicaSnapshot<TData>)
			: nextRaw;
		if (!snapshotEqual(this.#snapshot, next)) {
			this.#snapshot = next;
			const errors: unknown[] = [];
			for (const listener of this.#listeners) {
				try {
					listener(next);
				} catch (error) {
					errors.push(error);
				}
			}
			this.#owner._reportObserverErrors(errors);
		}
		if (allowFetch) void this.#owner._fetch(this, false);
	}
}

export function createDistributedReplica(
	options: DistributedReplicaOptions = {}
): DistributedReplicaApi {
	return new DistributedReplicaImpl(options);
}

function optimisticWriter(writer: OptimisticCacheWriter): ReplicaOptimisticWriter {
	return Object.freeze({
		writeRecord(
			model: ReplicaModelArtifact,
			identity: ReplicaIdentity,
			patch: ReplicaRecordPatch
		): void {
			writer.writeRecord({ key: replicaRecordKey(model, identity), ...patch });
		},
		tombstoneRecord(model: ReplicaModelArtifact, identity: ReplicaIdentity): void {
			writer.tombstoneRecord(replicaRecordKey(model, identity));
		},
		writeIndex(target: ReplicaIndexTarget, records: readonly string[]): void {
			writer.writeIndex({
				key: indexKeyFromTarget(target),
				records,
				complete: target.complete ?? false,
				metadata: metadataFromTarget(target)
			});
		},
		deleteIndex(target: ReplicaIndexTarget): void {
			writer.deleteIndex(indexKeyFromTarget(target));
		}
	});
}

function baseWriter(writer: BaseCacheWriter): ReplicaBaseWriter {
	return Object.freeze({
		writeRecord(
			model: ReplicaModelArtifact,
			identity: ReplicaIdentity,
			revision: ReplicaRevision,
			patch: ReplicaRecordPatch & { readonly incarnation?: ReplicaRevision }
		): boolean {
			return writer.writeRecord({
				key: replicaRecordKey(model, identity),
				revision,
				...patch
			});
		},
		tombstoneRecord(
			model: ReplicaModelArtifact,
			identity: ReplicaIdentity,
			revision: ReplicaRevision
		): boolean {
			return writer.tombstoneRecord(replicaRecordKey(model, identity), revision);
		},
		writeIndex(
			target: ReplicaIndexTarget,
			records: readonly string[],
			revision: ReplicaRevision
		): boolean {
			return writer.writeIndex({
				key: indexKeyFromTarget(target),
				revision,
				records,
				complete: target.complete ?? false,
				metadata: metadataFromTarget(target)
			});
		},
		deleteIndex(target: ReplicaIndexTarget, revision: ReplicaRevision): boolean {
			return writer.deleteIndex(indexKeyFromTarget(target), revision);
		}
	});
}

function metadataFromTarget(target: ReplicaIndexTarget): CacheIndexMetadata {
	const dependencies = [...new Set(target.dependencies ?? [])].sort();
	return Object.freeze({
		...(target.parent === undefined ? {} : { parent: target.parent }),
		field: target.field,
		arguments: target.arguments ?? Object.freeze({}),
		coverage: target.coverage ?? ({ kind: 'unknown' } as CacheIndexCoverage),
		dependencies: Object.freeze(dependencies),
		...(target.staleReason === undefined ? {} : { staleReason: target.staleReason }),
		...(target.nullValue === undefined ? {} : { nullValue: target.nullValue })
	});
}

function indexKeyFromTarget(target: ReplicaIndexTarget): string {
	return replicaIndexKey({
		...(target.parent === undefined ? {} : { parent: target.parent }),
		field: target.field,
		arguments: target.arguments ?? {}
	});
}

function operationKey<TData, TVariables extends GraphqlVariables>(
	artifact: ReplicaOperationArtifact<TData, TVariables>,
	variables: TVariables
): string {
	if (typeof artifact.id !== 'string' || artifact.id.length === 0) {
		throw new TypeError('replica artifact id must be a non-empty string');
	}
	return `${artifact.id}:${canonicalVariables(variables)}`;
}

function snapshotFrom<TData>(
	materialized: MaterializedReplicaResult<TData>,
	state: QueryState
): ReplicaSnapshot<TData> {
	const status: ReplicaStatus =
		state.errors.length > 0
			? 'error'
			: materialized.stale
				? 'stale'
				: materialized.complete
					? 'ready'
					: 'loading';
	const snapshot = Object.freeze({
		data: materialized.data,
		status,
		fetching: state.fetching,
		stale: materialized.stale,
		complete: materialized.complete,
		errors: state.errors,
		live: state.live
	});
	// `materializeReplicaOperation` only reports complete after every generated
	// selection is present; that runtime invariant is what promotes sparse data
	// to the generated result type in ReplicaSnapshot's discriminated union.
	return snapshot as ReplicaSnapshot<TData>;
}

function snapshotEqual<TData>(
	left: ReplicaSnapshot<TData>,
	right: ReplicaSnapshot<TData>
): boolean {
	return (
		left.data === right.data &&
		left.status === right.status &&
		left.fetching === right.fetching &&
		left.stale === right.stale &&
		left.complete === right.complete &&
		left.errors === right.errors &&
		left.live === right.live
	);
}

function freezeErrors(errors: readonly GqlError[]): readonly GqlError[] {
	if (errors.length === 0) return EMPTY_ERRORS;
	return Object.freeze(
		errors.map((error) =>
			(Object.freeze({
				message: error.message,
				...(error.locations === undefined
					? {}
					: {
							locations: Object.freeze(
								error.locations.map((location) => Object.freeze({ ...location }))
							)
						}),
				...(error.path === undefined ? {} : { path: Object.freeze([...error.path]) }),
				...(error.extensions === undefined
					? {}
					: {
							extensions: cloneJsonValue(error.extensions) as GqlError['extensions']
						})
			}) as GqlError)
		)
	);
}

function stableErrors(
	current: readonly GqlError[],
	next: readonly GqlError[]
): readonly GqlError[] {
	if (next.length === 0) return EMPTY_ERRORS;
	return deepEqual(current, next) ? current : freezeErrors(next);
}

function graphqlError(error: unknown): GqlError {
	return Object.freeze({
		message: error instanceof Error ? error.message : String(error),
		extensions: Object.freeze({ code: 'REPLICA_TRANSPORT' })
	});
}

function assertWriteSource(source: ReplicaWriteSource): void {
	if (!['network', 'live', 'ssr', 'restore', 'projected'].includes(source)) {
		throw new TypeError(`unsupported replica write source: ${source}`);
	}
}

function revisionToken(value: ReplicaRevision): bigint {
	if (typeof value === 'bigint') {
		if (value < 0n) throw new TypeError('replica revision must be unsigned');
		return value;
	}
	if (typeof value === 'number') {
		if (!Number.isSafeInteger(value) || value < 0) {
			throw new TypeError('numeric replica revision must be an unsigned safe integer');
		}
		return BigInt(value);
	}
	if (!/^(0|[1-9][0-9]*)$/.test(value)) {
		throw new TypeError('string replica revision must be a canonical unsigned integer');
	}
	return BigInt(value);
}

function deepEqual(left: unknown, right: unknown): boolean {
	if (Object.is(left, right)) return true;
	if (typeof left !== typeof right || left === null || right === null) return false;
	if (typeof left !== 'object' || typeof right !== 'object') return false;
	if (Array.isArray(left) || Array.isArray(right)) {
		if (!Array.isArray(left) || !Array.isArray(right) || left.length !== right.length) return false;
		return left.every((entry, index) => deepEqual(entry, right[index]));
	}
	const leftRecord = left as Readonly<Record<string, unknown>>;
	const rightRecord = right as Readonly<Record<string, unknown>>;
	const leftKeys = Object.keys(leftRecord);
	const rightKeys = Object.keys(rightRecord);
	return (
		leftKeys.length === rightKeys.length &&
		leftKeys.every(
			(key) =>
				Object.prototype.hasOwnProperty.call(rightRecord, key) &&
				deepEqual(leftRecord[key], rightRecord[key])
		)
	);
}

function reportUnhandledObserverError(error: AggregateError): void {
	const reportError = (globalThis as { reportError?: (cause: unknown) => void }).reportError;
	if (typeof reportError === 'function') {
		reportError(error);
		return;
	}
	queueMicrotask(() => {
		throw error;
	});
}

function reportSafely(
	reporter: (error: AggregateError) => void,
	error: AggregateError
): void {
	try {
		reporter(error);
	} catch (reporterError) {
		queueMicrotask(() => {
			throw new AggregateError(
				[error, reporterError],
				'replica observer error reporter failed'
			);
		});
	}
}
