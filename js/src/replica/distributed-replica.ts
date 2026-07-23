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
	compareDistributedDecimal,
	DistributedProtocolError,
	parseGraphqlResponseExtensions,
	type DistributedCommandMetadata,
	type DistributedDecimalString,
	type DistributedIndexRevision,
	type DistributedLiveCursor,
	type DistributedOpaqueString,
	type DistributedProjectionObservation,
	type DistributedProtocolEnvelope,
	type DistributedQuerySnapshot,
	type DistributedRecordRevision
} from '../protocol.js';
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
import {
	normalizeReplicaResult,
	type ReplicaNormalizationProtocol,
	type ReplicaProtocolRecordResolution
} from './normalize.js';
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
	protocolGeneration: number;
	operationGeneration?: number;
};

type ProtocolGeneration = {
	cacheScope: DistributedOpaqueString;
	schemaHash: string;
};

type RecordProtocolClock = {
	scopeToken: DistributedOpaqueString;
	incarnation: DistributedDecimalString;
	revision: DistributedDecimalString;
	tombstone: boolean;
};

type AnonymousRecordProtocolClock = {
	model: string;
	clock: RecordProtocolClock;
};

type IndexProtocolClock = {
	scopeToken: DistributedOpaqueString;
	position: DistributedDecimalString;
};

type OperationProtocolState = {
	operation: string;
	snapshotScope?: DistributedOpaqueString;
	indexClocks: Map<string, IndexProtocolClock>;
	indexRevision?: string;
	indexKeys: Set<string>;
	pathRecords: Map<string, string>;
	cursors: readonly DistributedLiveCursor[];
};

type OperationProtocolSource = 'query' | 'live';

type OperationProtocolGroup = {
	query?: OperationProtocolState;
	live?: OperationProtocolState;
	active?: OperationProtocolSource;
};

type OptimisticReceiptState = {
	causationId: DistributedOpaqueString;
	expectations: ReadonlyMap<string, true>;
	observed: Set<string>;
};

type IndexDisposition = 'fresh' | 'equal' | 'higher' | 'lower' | 'incomparable';

const EMPTY_ERRORS: readonly GqlError[] = Object.freeze([]);
/** Matches protocol.ts MAX_EVIDENCE_ITEMS without making it public API. */
const MAX_ANONYMOUS_RECORD_CLOCKS = 4_096;
const EMPTY_CACHE_SNAPSHOT = Object.freeze({
	version: 1 as const,
	records: Object.freeze([]),
	indexes: Object.freeze([])
});

class DistributedReplicaImpl implements DistributedReplicaApi {
	readonly #engine: CacheEngine;
	readonly #transport: ReplicaTransport | undefined;
	readonly #reportObserverError: (error: AggregateError) => void;
	readonly #inFlight = new Map<string, Promise<void>>();
	readonly #queryStates = new Map<string, QueryState>();
	readonly #watches = new Map<string, Set<ReplicaWatchState<unknown, GraphqlVariables>>>();
	readonly #lives = new Map<string, LiveEntry>();
	readonly #operationProtocols = new Map<string, OperationProtocolGroup>();
	readonly #operationGenerations = new Map<string, number>();
	readonly #recordClocks = new Map<string, RecordProtocolClock>();
	readonly #recordKeysByScope = new Map<DistributedOpaqueString, string>();
	readonly #anonymousRecordClocks = new Map<
		DistributedOpaqueString,
		AnonymousRecordProtocolClock
	>();
	readonly #optimisticReceipts = new Map<string, OptimisticReceiptState>();
	#protocolGeneration: ProtocolGeneration | undefined;
	#protocolGenerationSequence = 0;
	#nextIndexRevision = '0';

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
		const extensions = parseGraphqlResponseExtensions(envelope.extensions);
		const parsedEnvelope: ReplicaResultEnvelope<TData> = Object.freeze({
			...envelope,
			...(extensions === undefined ? {} : { extensions })
		});
		const stableVariables = cloneJsonObject(variables) as TVariables;
		const key = operationKey(artifact, stableVariables);
		const distributed = extensions?.distributed;
		if (distributed) {
			this.#writeProtocolResult(
				key,
				artifact,
				stableVariables,
				parsedEnvelope,
				source,
				distributed
			);
			return;
		}
		if (artifact.protocol !== undefined) {
			protocolInvalid('extensions.distributed');
		}
		this.#writeLegacyResult(key, artifact, stableVariables, parsedEnvelope);
	}

	#writeLegacyResult<TData, TVariables extends GraphqlVariables>(
		key: string,
		artifact: ReplicaOperationArtifact<TData, TVariables>,
		stableVariables: TVariables,
		envelope: ReplicaResultEnvelope<TData>
	): void {
		if (envelope.revision === undefined) {
			throw new TypeError('noncausal replica results require a legacy revision');
		}
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

	#writeProtocolResult<TData, TVariables extends GraphqlVariables>(
		key: string,
		artifact: ReplicaOperationArtifact<TData, TVariables>,
		stableVariables: TVariables,
		envelope: ReplicaResultEnvelope<TData>,
		source: ReplicaWriteSource,
		distributed: DistributedProtocolEnvelope
	): void {
		this.#validateProtocolBinding(artifact, distributed, source);
		this.#adoptProtocolGeneration(distributed);

		const snapshot = distributed.snapshot;
		const live = distributed.live;
		if (live !== undefined && snapshot === undefined) {
			protocolInvalid('extensions.distributed.snapshot');
		}
		if (
			source === 'live' &&
			(envelope.data !== undefined || envelope.errors !== undefined) &&
			(snapshot === undefined || live === undefined)
		) {
			protocolInvalid('extensions.distributed.live');
		}

		const operation = distributed.operation;
		if ((snapshot !== undefined || live !== undefined) && operation === undefined) {
			protocolInvalid('extensions.distributed.operation');
		}
		const operationSource = protocolOperationSource(source);
		const operationState =
			operation === undefined
				? undefined
				: this.#operationProtocol(key, operation, operationSource);
		if (snapshot === undefined || operationState === undefined) {
			this.#applyReceiptOnly(distributed.command);
			return;
		}

		this.#validateLiveSnapshot(snapshot, live);
		const reset = live?.reset === true;
		const group = this.#operationProtocols.get(key)!;
		const previousActiveSource = group.active;
		const handoff =
			previousActiveSource !== undefined &&
			previousActiveSource !== operationSource;
		const activeState =
			previousActiveSource === undefined
				? undefined
				: group[previousActiveSource];
		const ownDisposition = reset
			? 'fresh'
			: compareSnapshotToOperationState(operationState, snapshot);
		const activeDisposition =
			handoff && activeState !== undefined
				? compareSnapshotToOperationState(activeState, snapshot)
				: 'fresh';
		const handoffBlocked =
			handoff &&
			(
				!snapshot.complete ||
				!isComparableHandoffDisposition(ownDisposition) ||
				!isComparableHandoffDisposition(activeDisposition)
			);
		let disposition: IndexDisposition = handoffBlocked
			? !isComparableHandoffDisposition(activeDisposition)
				? activeDisposition
				: !isComparableHandoffDisposition(ownDisposition)
					? ownDisposition
					: 'incomparable'
			: ownDisposition;
		const sourceSwitched =
			!handoffBlocked &&
			isComparableHandoffDisposition(disposition) &&
			this.#activateOperationSource(
				key,
				operationSource,
				artifact,
				stableVariables
			);
		const rejectedHandoff = handoff && !sourceSwitched;
		if (reset || ownDisposition === 'incomparable') {
			if (rejectedHandoff) {
				this.#resetOperationState(operationState);
			} else {
				this.#discardOperationSnapshot(
					operationState,
					artifact,
					stableVariables
				);
			}
		}
		if (sourceSwitched && disposition !== 'incomparable') {
			// Query and live operation hashes describe independent protocol
			// streams. A handoff is authoritative, but must receive a fresh local
			// index revision rather than reusing the inactive stream's revision.
			disposition = 'fresh';
		}

		if (!snapshot.complete) {
			if (rejectedHandoff) {
				this.#resetOperationState(operationState);
			} else {
				this.#discardOperationSnapshot(
					operationState,
					artifact,
					stableVariables
				);
			}
			disposition = 'incomparable';
		} else if (disposition === 'incomparable') {
			if (rejectedHandoff) {
				this.#resetOperationState(operationState);
			} else {
				this.#discardOperationSnapshot(
					operationState,
					artifact,
					stableVariables
				);
			}
		}

		const writeIndexes =
			snapshot.complete &&
			disposition !== 'lower' &&
			disposition !== 'incomparable';
		const indexRevision =
			disposition === 'equal' && operationState.indexRevision !== undefined
				? operationState.indexRevision
				: this.#allocateIndexRevision();
		const recordEvidence = prepareRecordEvidence(
			snapshot,
			distributed.command?.records ?? Object.freeze([])
		);
		const pendingRecordClocks = new Map<string, RecordProtocolClock>();
		const pendingRecordScopes = new Map<DistributedOpaqueString, string>();
		const pendingAnonymousRecordClocks = new Map<
			DistributedOpaqueString,
			AnonymousRecordProtocolClock
		>();
		const consumedAnonymousRecordClocks = new Set<
			DistributedOpaqueString
		>();
		const pendingPathRecords = new Map<string, string>();
		const consumedRecordPaths = new Set<string>();
		const observationsAdmissible =
			snapshot.complete &&
				disposition !== 'incomparable' &&
				(disposition !== 'lower' ||
					this.#operationProtocols.get(key)?.active ===
						operationSource);
		const receiptPlan = this.#planOptimisticReceipts(
			distributed.command,
			[
				...(distributed.command?.observations ?? []),
				...(observationsAdmissible ? snapshot.observations : [])
			],
			observationsAdmissible
		);
		const normalizationProtocol: ReplicaNormalizationProtocol = {
			indexRevision,
			writeIndexes,
			indexesComplete: snapshot.complete,
			record: (
				path,
				model,
				recordKey
			): ReplicaProtocolRecordResolution | undefined => {
				const encodedPath = responsePathKey(path);
				const evidence = recordEvidence.byPath.get(encodedPath);
				if (evidence === undefined) {
					if (snapshot.complete) {
						protocolInvalid(
							'extensions.distributed.snapshot.records'
						);
					}
					return undefined;
				}
				if (evidence.model !== model) {
					protocolInvalid(
						'extensions.distributed.snapshot.records.model'
					);
				}
				if (evidence.tombstone) {
					protocolInvalid(
						'extensions.distributed.snapshot.records.tombstone'
					);
				}
				consumedRecordPaths.add(encodedPath);
				const resolution = this.#resolveRecordEvidence(
					recordKey,
					evidence,
					pendingRecordClocks,
					pendingRecordScopes,
					pendingAnonymousRecordClocks,
					consumedAnonymousRecordClocks
				);
				pendingPathRecords.set(encodedPath, recordKey);
				return Object.freeze({ evidence, apply: resolution });
			}
		};

		const state = this.#queryState(key);
		const previousErrors = state.errors;
		state.errors = stableErrors(previousErrors, envelope.errors ?? []);
		let summary: ReturnType<typeof normalizeReplicaResult>;
		try {
			const update = (writer: BaseCacheWriter) => {
				this.#applyTombstoneEvidence(
					writer,
					recordEvidence.tombstones,
					operationState,
					pendingRecordClocks,
					pendingRecordScopes,
					pendingAnonymousRecordClocks,
					consumedAnonymousRecordClocks,
					consumedRecordPaths
				);
				const normalized = normalizeReplicaResult(
					writer,
					artifact,
					stableVariables,
					envelope,
					normalizationProtocol
				);
				for (const path of recordEvidence.livePaths) {
					if (!consumedRecordPaths.has(path)) {
						protocolInvalid(
							'extensions.distributed.snapshot.records.path'
						);
					}
				}
				this.#applyPathlessEvidence(
					writer,
					recordEvidence.pathless,
					recordEvidence.byPath,
					consumedRecordPaths,
					pendingRecordClocks,
					pendingRecordScopes,
					pendingAnonymousRecordClocks,
					consumedAnonymousRecordClocks
				);
				return normalized;
			};
			summary =
				receiptPlan.satisfied.length === 0
					? this.#engine.batch(update)
					: this.#engine.confirmOptimisticLayers(
							receiptPlan.satisfied,
							update
						);
		} catch (error) {
			state.errors = previousErrors;
			if (
				error instanceof CacheRevisionConflictError ||
				error instanceof DistributedProtocolError
			) {
				this.#discardOperationSnapshot(
					operationState,
					artifact,
					stableVariables
				);
			}
			this.#emitState(key, false);
			throw error;
		}

		for (const [recordKey, clock] of pendingRecordClocks) {
			this.#recordClocks.set(recordKey, clock);
			this.#recordKeysByScope.set(clock.scopeToken, recordKey);
		}
		for (const [scopeToken, recordKey] of pendingRecordScopes) {
			this.#recordKeysByScope.set(scopeToken, recordKey);
		}
		for (const [scopeToken, clock] of pendingAnonymousRecordClocks) {
			if (!consumedAnonymousRecordClocks.has(scopeToken)) {
				this.#anonymousRecordClocks.set(scopeToken, clock);
			}
		}
		for (const scopeToken of consumedAnonymousRecordClocks) {
			this.#anonymousRecordClocks.delete(scopeToken);
		}
		for (const [path, recordKey] of pendingPathRecords) {
			operationState.pathRecords.set(path, recordKey);
		}
		for (const [id, receipt] of receiptPlan.updates) {
			if (receiptPlan.satisfied.includes(id)) {
				this.#optimisticReceipts.delete(id);
			} else {
				this.#optimisticReceipts.set(id, receipt);
				this.#engine.markOptimisticLayerAccepted(id);
			}
		}
		if (writeIndexes) {
			operationState.snapshotScope = snapshot.scopeToken;
			operationState.indexClocks = indexClockMap(snapshot.indexes);
			operationState.indexRevision = indexRevision;
			for (const indexKey of summary.indexKeys) {
				operationState.indexKeys.add(indexKey);
			}
			operationState.cursors = latestCursors(snapshot, live);
		} else if (live?.reset === true || !snapshot.complete) {
			operationState.cursors = Object.freeze([]);
		}
		if (source === 'live') {
			this.#advanceOperationGeneration(key);
		}
		if (source !== 'live' && sourceSwitched) {
			this.#restartLive(key);
		}
		this.#emitState(key, false);
	}

	createOptimisticLayer(
		id: string,
		update: (writer: ReplicaOptimisticWriter) => void
	): void {
		this.#engine.createOptimisticLayer(id, (writer) => update(optimisticWriter(writer)));
	}

	markOptimisticLayerAccepted(
		id: string,
		receipt?: DistributedCommandMetadata
	): boolean {
		if (receipt !== undefined && receipt.commandId !== id) {
			throw new TypeError('optimistic layer id must equal the causal command id');
		}
		const accepted = this.#engine.markOptimisticLayerAccepted(id);
		if (!accepted || receipt === undefined) return accepted;
		const next = optimisticReceiptState(receipt);
		const current = this.#optimisticReceipts.get(id);
		if (current !== undefined && !sameReceipt(current, next)) {
			throw new DistributedProtocolError(
				'DISTRIBUTED_PROTOCOL_INVALID',
				'extensions.distributed.command'
			);
		}
		this.#optimisticReceipts.set(id, next);
		return true;
	}

	confirmOptimisticLayer<T>(
		id: string,
		update: (writer: ReplicaBaseWriter) => T
	): T {
		const result = this.#engine.confirmOptimisticLayer(id, (writer) =>
			update(baseWriter(writer))
		);
		this.#optimisticReceipts.delete(id);
		return result;
	}

	rejectOptimisticLayer(id: string): boolean {
		const rejected = this.#engine.rejectOptimisticLayer(id);
		if (rejected) this.#optimisticReceipts.delete(id);
		return rejected;
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

	#validateProtocolBinding<TData, TVariables extends GraphqlVariables>(
		artifact: ReplicaOperationArtifact<TData, TVariables>,
		envelope: DistributedProtocolEnvelope,
		source: ReplicaWriteSource
	): void {
		const binding = artifact.protocol;
		if (binding === undefined) {
			protocolInvalid('extensions.distributed');
		}
		if (binding.version !== 2) {
			throw new TypeError('replica artifact protocol version is unsupported');
		}
		if (binding.schemaHash !== envelope.schemaHash) {
			this.#purgeProtocolGeneration();
			protocolInvalid('extensions.distributed.schemaHash');
		}
		const expectedOperation =
			source === 'live' ? artifact.live?.id : binding.operation;
		if (
			expectedOperation === undefined ||
			envelope.operation !== expectedOperation
		) {
			protocolInvalid('extensions.distributed.operation');
		}
	}

	#adoptProtocolGeneration(envelope: DistributedProtocolEnvelope): void {
		const next: ProtocolGeneration = {
			cacheScope: envelope.cacheScope,
			schemaHash: envelope.schemaHash
		};
		if (this.#protocolGeneration === undefined) {
			this.#protocolGeneration = next;
			return;
		}
		if (
			this.#protocolGeneration.cacheScope === next.cacheScope &&
			this.#protocolGeneration.schemaHash === next.schemaHash
		) {
			return;
		}
		this.#purgeProtocolGeneration();
		this.#protocolGeneration = next;
	}

	#purgeProtocolGeneration(): void {
		this.#protocolGenerationSequence += 1;
		for (const entry of this.#lives.values()) {
			entry.active = false;
			try {
				entry.unsubscribe();
			} catch {
				// The generation fence is already closed; transport cleanup is best effort.
			}
		}
		this.#lives.clear();
		this.#inFlight.clear();
		this.#queryStates.clear();
		this.#operationProtocols.clear();
		this.#operationGenerations.clear();
		this.#recordClocks.clear();
		this.#recordKeysByScope.clear();
		this.#anonymousRecordClocks.clear();
		this.#optimisticReceipts.clear();
		this.#nextIndexRevision = '0';
		this.#protocolGeneration = undefined;
		this.#engine.restore(EMPTY_CACHE_SNAPSHOT);
	}

	#operationProtocol(
		key: string,
		operation: string,
		source: OperationProtocolSource
	): OperationProtocolState {
		let group = this.#operationProtocols.get(key);
		if (group === undefined) {
			group = {};
			this.#operationProtocols.set(key, group);
		}
		const current = group[source];
		if (current !== undefined) {
			if (current.operation !== operation) {
				protocolInvalid('extensions.distributed.operation');
			}
			return current;
		}
		const created: OperationProtocolState = {
			operation,
			indexClocks: new Map(),
			indexKeys: new Set(),
			pathRecords: new Map(),
			cursors: Object.freeze([])
		};
		group[source] = created;
		return created;
	}

	#activateOperationSource<TData, TVariables extends GraphqlVariables>(
		key: string,
		source: OperationProtocolSource,
		artifact: ReplicaOperationArtifact<TData, TVariables>,
		variables: TVariables
	): boolean {
		const group = this.#operationProtocols.get(key);
		if (group === undefined) return false;
		const previous = group.active;
		if (previous === source) return false;
		if (previous !== undefined) {
			const keys = new Set<string>();
			for (const state of [group.query, group.live]) {
				for (const indexKey of state?.indexKeys ?? []) keys.add(indexKey);
			}
			for (const root of artifact.roots) {
				keys.add(
					replicaIndexKey({
						field: root.field,
						arguments: resolveArguments(root.arguments, variables)
					})
				);
			}
			this.#engine.discardIndexes([...keys]);
		}
		group.active = source;
		this.#advanceOperationGeneration(key);
		return previous !== undefined;
	}

	#operationGeneration(key: string): number {
		return this.#operationGenerations.get(key) ?? 0;
	}

	#advanceOperationGeneration(key: string): void {
		this.#operationGenerations.set(
			key,
			this.#operationGeneration(key) + 1
		);
	}

	#resumeCursors(key: string): readonly DistributedLiveCursor[] {
		const group = this.#operationProtocols.get(key);
		if (group?.active === 'query' && group.query?.cursors.length) {
			return group.query.cursors;
		}
		if (group?.active === 'live' && group.live?.cursors.length) {
			return group.live.cursors;
		}
		return group?.live?.cursors.length
			? group.live.cursors
			: (group?.query?.cursors ?? Object.freeze([]));
	}

	#discardOperationSnapshot<TData, TVariables extends GraphqlVariables>(
		state: OperationProtocolState,
		artifact: ReplicaOperationArtifact<TData, TVariables>,
		variables: TVariables
	): void {
		const keys = new Set(state.indexKeys);
		for (const root of artifact.roots) {
			keys.add(
				replicaIndexKey({
					field: root.field,
					arguments: resolveArguments(root.arguments, variables)
				})
			);
		}
		this.#engine.discardIndexes([...keys]);
		this.#resetOperationState(state);
	}

	#resetOperationState(state: OperationProtocolState): void {
		state.snapshotScope = undefined;
		state.indexClocks = new Map();
		state.indexRevision = undefined;
		state.indexKeys.clear();
		state.pathRecords.clear();
		state.cursors = Object.freeze([]);
	}

	#allocateIndexRevision(): string {
		this.#nextIndexRevision = incrementCanonicalDecimal(
			this.#nextIndexRevision
		);
		return this.#nextIndexRevision;
	}

	#resolveRecordEvidence(
		recordKey: string,
		evidence: DistributedRecordRevision,
		pendingClocks: Map<string, RecordProtocolClock>,
		pendingScopes: Map<DistributedOpaqueString, string>,
		pendingAnonymousClocks: Map<
			DistributedOpaqueString,
			AnonymousRecordProtocolClock
		>,
		consumedAnonymousClocks: Set<DistributedOpaqueString>
	): boolean {
		if (!recordKeyMatchesModel(recordKey, evidence.model)) {
			protocolInvalid('extensions.distributed.snapshot.records.model');
		}
		const scopedKey =
			pendingScopes.get(evidence.scopeToken) ??
			this.#recordKeysByScope.get(evidence.scopeToken);
		if (scopedKey !== undefined && scopedKey !== recordKey) {
			protocolInvalid('extensions.distributed.snapshot.records.scopeToken');
		}
		pendingScopes.set(evidence.scopeToken, recordKey);

		const incoming: RecordProtocolClock = {
			scopeToken: evidence.scopeToken,
			incarnation: evidence.incarnation,
			revision: evidence.revision,
			tombstone: evidence.tombstone
		};
		const pending = pendingClocks.get(recordKey);
		if (pending !== undefined) {
			if (!sameRecordClock(pending, incoming)) {
				protocolInvalid('extensions.distributed.snapshot.records');
			}
			return true;
		}
		let current = this.#recordClocks.get(recordKey);
		const anonymous =
			pendingAnonymousClocks.get(evidence.scopeToken) ??
			this.#anonymousRecordClocks.get(evidence.scopeToken);
		if (anonymous !== undefined) {
			if (anonymous.model !== evidence.model) {
				protocolInvalid(
					'extensions.distributed.snapshot.records.model'
				);
			}
			consumedAnonymousClocks.add(evidence.scopeToken);
			if (current === undefined) {
				current = anonymous.clock;
			} else {
				if (current.scopeToken !== anonymous.clock.scopeToken) {
					protocolInvalid(
						'extensions.distributed.snapshot.records.scopeToken'
					);
				}
				const anonymousComparison = compareRecordClock(
					anonymous.clock,
					current
				);
				if (
					anonymousComparison === 0 &&
					anonymous.clock.tombstone !== current.tombstone
				) {
					protocolInvalid(
						'extensions.distributed.snapshot.records.tombstone'
					);
				}
				if (anonymousComparison > 0) current = anonymous.clock;
			}
		}
		if (current === undefined) {
			pendingClocks.set(recordKey, incoming);
			return true;
		}
		if (current.scopeToken !== incoming.scopeToken) {
			protocolInvalid('extensions.distributed.snapshot.records.scopeToken');
		}
		const comparison = compareRecordClock(incoming, current);
		if (comparison < 0) {
			pendingClocks.set(recordKey, current);
			return false;
		}
		if (comparison === 0) {
			if (current.tombstone !== incoming.tombstone) {
				protocolInvalid('extensions.distributed.snapshot.records.tombstone');
			}
			pendingClocks.set(recordKey, incoming);
			return true;
		}
		if (
			current.tombstone &&
			!incoming.tombstone &&
			compareDistributedDecimal(
				incoming.incarnation,
				current.incarnation
			) <= 0
		) {
			protocolInvalid('extensions.distributed.snapshot.records.incarnation');
		}
		pendingClocks.set(recordKey, incoming);
		return true;
	}

	#retainAnonymousRecordEvidence(
		evidence: DistributedRecordRevision,
		pendingAnonymousClocks: Map<
			DistributedOpaqueString,
			AnonymousRecordProtocolClock
		>
	): void {
		const incoming: AnonymousRecordProtocolClock = {
			model: evidence.model,
			clock: {
				scopeToken: evidence.scopeToken,
				incarnation: evidence.incarnation,
				revision: evidence.revision,
				tombstone: evidence.tombstone
			}
		};
		const current =
			pendingAnonymousClocks.get(evidence.scopeToken) ??
			this.#anonymousRecordClocks.get(evidence.scopeToken);
		if (current === undefined) {
			let retained = this.#anonymousRecordClocks.size;
			for (const scopeToken of pendingAnonymousClocks.keys()) {
				if (!this.#anonymousRecordClocks.has(scopeToken)) retained += 1;
			}
			if (retained >= MAX_ANONYMOUS_RECORD_CLOCKS) {
				protocolInvalid(
					'extensions.distributed.snapshot.records.capacity'
				);
			}
			pendingAnonymousClocks.set(evidence.scopeToken, incoming);
			return;
		}
		if (current.model !== incoming.model) {
			protocolInvalid('extensions.distributed.snapshot.records.model');
		}
		const comparison = compareRecordClock(
			incoming.clock,
			current.clock
		);
		if (comparison < 0) return;
		if (comparison === 0) {
			if (current.clock.tombstone !== incoming.clock.tombstone) {
				protocolInvalid(
					'extensions.distributed.snapshot.records.tombstone'
				);
			}
			return;
		}
		if (
			current.clock.tombstone &&
			!incoming.clock.tombstone &&
			compareDistributedDecimal(
				incoming.clock.incarnation,
				current.clock.incarnation
			) <= 0
		) {
			protocolInvalid(
				'extensions.distributed.snapshot.records.incarnation'
			);
		}
		pendingAnonymousClocks.set(evidence.scopeToken, incoming);
	}

	#applyTombstoneEvidence(
		writer: BaseCacheWriter,
		evidenceItems: readonly DistributedRecordRevision[],
		operation: OperationProtocolState,
		pendingClocks: Map<string, RecordProtocolClock>,
		pendingScopes: Map<DistributedOpaqueString, string>,
		pendingAnonymousClocks: Map<
			DistributedOpaqueString,
			AnonymousRecordProtocolClock
		>,
		consumedAnonymousClocks: Set<DistributedOpaqueString>,
		consumedPaths: Set<string>
	): void {
		for (const evidence of evidenceItems) {
			const encodedPath =
				evidence.path === undefined
					? undefined
					: responsePathKey(evidence.path);
			const recordKey =
				this.#recordKeysByScope.get(evidence.scopeToken) ??
				(encodedPath === undefined
					? undefined
					: operation.pathRecords.get(encodedPath));
			if (recordKey === undefined) {
				this.#retainAnonymousRecordEvidence(
					evidence,
					pendingAnonymousClocks
				);
				continue;
			}
			if (encodedPath !== undefined) consumedPaths.add(encodedPath);
			if (
				this.#resolveRecordEvidence(
					recordKey,
					evidence,
					pendingClocks,
					pendingScopes,
					pendingAnonymousClocks,
					consumedAnonymousClocks
				)
			) {
				writer.tombstoneRecord(
					recordKey,
					evidence.revision,
					evidence.incarnation
				);
			}
		}
	}

	#applyPathlessEvidence(
		writer: BaseCacheWriter,
		evidenceItems: readonly DistributedRecordRevision[],
		pathEvidence: ReadonlyMap<string, DistributedRecordRevision>,
		consumedPaths: ReadonlySet<string>,
		pendingClocks: Map<string, RecordProtocolClock>,
		pendingScopes: Map<DistributedOpaqueString, string>,
		pendingAnonymousClocks: Map<
			DistributedOpaqueString,
			AnonymousRecordProtocolClock
		>,
		consumedAnonymousClocks: Set<DistributedOpaqueString>
	): void {
		for (const evidence of evidenceItems) {
			const recordKey =
				pendingScopes.get(evidence.scopeToken) ??
				this.#recordKeysByScope.get(evidence.scopeToken);
			if (recordKey === undefined) {
				this.#retainAnonymousRecordEvidence(
					evidence,
					pendingAnonymousClocks
				);
				continue;
			}
			if (!recordKeyMatchesModel(recordKey, evidence.model)) {
				protocolInvalid(
					'extensions.distributed.snapshot.records.model'
				);
			}
			let certifiedByPath = false;
			for (const [path, candidate] of pathEvidence) {
				if (
					consumedPaths.has(path) &&
					candidate.scopeToken === evidence.scopeToken &&
					sameRecordRevision(candidate, evidence)
				) {
					certifiedByPath = true;
					break;
				}
			}
			if (certifiedByPath) continue;
			if (
				this.#resolveRecordEvidence(
					recordKey,
					evidence,
					pendingClocks,
					pendingScopes,
					pendingAnonymousClocks,
					consumedAnonymousClocks
				)
			) {
				// A change-log upsert proves only that a newer row exists. It
				// advances the tuple fence, but cannot certify any cached field.
				writer.discardRecord(recordKey);
			}
		}
	}

	#validateLiveSnapshot(
		snapshot: DistributedQuerySnapshot,
		live: DistributedProtocolEnvelope['live']
	): void {
		if (live === undefined || !live.supported) return;
		const indexes = new Map(
			snapshot.indexes.map((index) => [index.projection, index])
		);
		if (indexes.size !== live.cursors.length) {
			protocolInvalid('extensions.distributed.live.cursors');
		}
		for (const cursor of live.cursors) {
			const index = indexes.get(cursor.projection);
			if (
				index === undefined ||
				index.position !== cursor.position ||
				(index.resume !== undefined &&
					index.resume.token !== cursor.token)
			) {
				protocolInvalid('extensions.distributed.live.cursors');
			}
		}
	}

	#planOptimisticReceipts(
		command: DistributedCommandMetadata | undefined,
		observations: readonly DistributedProjectionObservation[],
		satisfactionAdmissible: boolean
	): {
		updates: Map<string, OptimisticReceiptState>;
		satisfied: string[];
	} {
		const updates = new Map<string, OptimisticReceiptState>();
		if (
			command !== undefined &&
			this.#engine.optimisticLayerState(command.commandId) !== undefined
		) {
			const proposed = optimisticReceiptState(command);
			const current = this.#optimisticReceipts.get(command.commandId);
			if (current !== undefined && !sameReceipt(current, proposed)) {
				protocolInvalid('extensions.distributed.command');
			}
			updates.set(
				command.commandId,
				cloneOptimisticReceipt(current ?? proposed)
			);
		}
		for (const [id, receipt] of this.#optimisticReceipts) {
			if (!updates.has(id)) {
				updates.set(id, cloneOptimisticReceipt(receipt));
			}
		}
		for (const receipt of updates.values()) {
			for (const observation of observations) {
				if (observation.causationId !== receipt.causationId) continue;
				const key = expectationKey(observation);
				if (receipt.expectations.has(key)) receipt.observed.add(key);
			}
		}
		const satisfied = satisfactionAdmissible
			? [...updates]
					.filter(
						([, receipt]) =>
							receipt.expectations.size > 0 &&
							[...receipt.expectations.keys()].every((key) =>
								receipt.observed.has(key)
							)
					)
					.map(([id]) => id)
			: [];
		return { updates, satisfied };
	}

	#applyReceiptOnly(command: DistributedCommandMetadata | undefined): void {
		if (command === undefined) return;
		const plan = this.#planOptimisticReceipts(
			command,
			command.observations,
			true
		);
		for (const [id, receipt] of plan.updates) {
			this.#optimisticReceipts.set(id, receipt);
			this.#engine.markOptimisticLayerAccepted(id);
		}
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
		const operationGeneration = this.#operationGeneration(watch.key);
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
				if (
					this.#operationGeneration(watch.key) !==
					operationGeneration
				) {
					return;
				}
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
		const entry: LiveEntry = {
			count: 1,
			unsubscribe: () => undefined,
			active: true,
			protocolGeneration: this.#protocolGenerationSequence
		};
		this.#lives.set(watch.key, entry);
		const resume = this.#resumeCursors(watch.key);
		try {
			const unsubscribe = this.#transport.subscribe(
				Object.freeze({
					operation: 'live' as const,
					operationId: watch.artifact.live.id,
					document: watch.artifact.live.document,
					variables: watch.variables,
					artifact: watch.artifact,
					...(resume === undefined || resume.length === 0
						? {}
						: { resume })
				}),
				{
					next: (result) => {
						if (
							!entry.active ||
							this.#lives.get(watch.key) !== entry ||
							entry.protocolGeneration !==
								this.#protocolGenerationSequence ||
							(entry.operationGeneration !== undefined &&
								entry.operationGeneration !==
									this.#operationGeneration(watch.key))
						) {
							return;
						}
						state.live = 'active';
						try {
							this.writeResult(watch.artifact, watch.variables, result, 'live');
							entry.operationGeneration =
								this.#operationGeneration(watch.key);
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

	#restartLive(key: string): void {
		const previous = this.#lives.get(key);
		if (previous === undefined) return;
		const count = previous.count;
		previous.active = false;
		this.#lives.delete(key);
		try {
			previous.unsubscribe();
		} catch {
			// The old generation is already fenced; cleanup is best effort.
		}
		const watch = [...(this.#watches.get(key) ?? [])].find(
			(candidate) => candidate.liveRequested
		);
		if (watch === undefined) {
			this.#queryState(key).live = 'off';
			return;
		}
		this.#retainLive(watch);
		const replacement = this.#lives.get(key);
		if (replacement !== undefined) replacement.count = count;
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

function prepareRecordEvidence(
	snapshot: DistributedQuerySnapshot,
	commandRecords: readonly DistributedRecordRevision[]
): {
	byPath: Map<string, DistributedRecordRevision>;
	tombstones: readonly DistributedRecordRevision[];
	pathless: readonly DistributedRecordRevision[];
	livePaths: ReadonlySet<string>;
} {
	const byPath = new Map<string, DistributedRecordRevision>();
	const tombstones: DistributedRecordRevision[] = [];
	const pathless: DistributedRecordRevision[] = [];
	const livePaths = new Set<string>();
	for (const evidence of snapshot.records) {
		if (evidence.path === undefined) {
			if (evidence.tombstone) tombstones.push(evidence);
			else pathless.push(evidence);
			continue;
		}
		const key = responsePathKey(evidence.path);
		if (byPath.has(key)) {
			protocolInvalid('extensions.distributed.snapshot.records.path');
		}
		byPath.set(key, evidence);
		if (evidence.tombstone) tombstones.push(evidence);
		else livePaths.add(key);
	}
	for (const evidence of commandRecords) {
		if (evidence.tombstone) tombstones.push(evidence);
		else pathless.push(evidence);
	}
	return {
		byPath,
		tombstones: Object.freeze(tombstones),
		pathless: Object.freeze(pathless),
		livePaths
	};
}

function sameRecordRevision(
	left: DistributedRecordRevision,
	right: DistributedRecordRevision
): boolean {
	return (
		left.model === right.model &&
		left.scopeToken === right.scopeToken &&
		left.incarnation === right.incarnation &&
		left.revision === right.revision &&
		left.tombstone === right.tombstone
	);
}

function recordKeyMatchesModel(recordKey: string, model: string): boolean {
	return recordKey.startsWith(`record:${encodeURIComponent(model)}:`);
}

function compareIndexVector(
	current: ReadonlyMap<string, IndexProtocolClock>,
	incoming: readonly DistributedIndexRevision[]
): IndexDisposition {
	if (current.size === 0) return 'fresh';
	if (current.size !== incoming.length) return 'incomparable';
	let lower = false;
	let higher = false;
	for (const evidence of incoming) {
		const previous = current.get(evidence.projection);
		if (
			previous === undefined ||
			previous.scopeToken !== evidence.scopeToken
		) {
			return 'incomparable';
		}
		const comparison = compareDistributedDecimal(
			evidence.position,
			previous.position
		);
		lower ||= comparison < 0;
		higher ||= comparison > 0;
	}
	if (lower && higher) return 'incomparable';
	if (lower) return 'lower';
	if (higher) return 'higher';
	return 'equal';
}

function compareSnapshotToOperationState(
	state: OperationProtocolState,
	snapshot: DistributedQuerySnapshot
): IndexDisposition {
	if (state.snapshotScope === undefined) return 'fresh';
	if (state.snapshotScope !== snapshot.scopeToken) return 'incomparable';
	if (state.indexClocks.size === 0 || snapshot.indexes.length === 0) {
		return state.indexClocks.size === snapshot.indexes.length
			? 'equal'
			: 'incomparable';
	}
	return compareIndexVector(state.indexClocks, snapshot.indexes);
}

function isComparableHandoffDisposition(
	disposition: IndexDisposition
): boolean {
	return (
		disposition === 'fresh' ||
		disposition === 'equal' ||
		disposition === 'higher'
	);
}

function indexClockMap(
	indexes: readonly DistributedIndexRevision[]
): Map<string, IndexProtocolClock> {
	return new Map(
		indexes.map((index) => [
			index.projection,
			{
				scopeToken: index.scopeToken,
				position: index.position
			}
		])
	);
}

function latestCursors(
	snapshot: DistributedQuerySnapshot,
	live: DistributedProtocolEnvelope['live']
): readonly DistributedLiveCursor[] {
	if (live !== undefined) {
		return live.supported ? live.cursors : Object.freeze([]);
	}
	return Object.freeze(
		snapshot.indexes.flatMap((index) =>
			index.resume === undefined ? [] : [index.resume]
		)
	);
}

function responsePathKey(path: readonly string[]): string {
	return JSON.stringify(path);
}

function compareRecordClock(
	left: RecordProtocolClock,
	right: RecordProtocolClock
): -1 | 0 | 1 {
	const incarnation = compareDistributedDecimal(
		left.incarnation,
		right.incarnation
	);
	return incarnation === 0
		? compareDistributedDecimal(left.revision, right.revision)
		: incarnation;
}

function sameRecordClock(
	left: RecordProtocolClock,
	right: RecordProtocolClock
): boolean {
	return (
		left.scopeToken === right.scopeToken &&
		left.incarnation === right.incarnation &&
		left.revision === right.revision &&
		left.tombstone === right.tombstone
	);
}

function optimisticReceiptState(
	command: DistributedCommandMetadata
): OptimisticReceiptState {
	const expectations = new Map<string, true>();
	for (const expectation of command.expects) {
		const key = expectationKey(expectation);
		if (expectations.has(key)) {
			protocolInvalid('extensions.distributed.command.expects');
		}
		expectations.set(key, true);
	}
	const observed = new Set<string>();
	for (const observation of command.observations) {
		if (observation.causationId !== command.causationId) {
			protocolInvalid('extensions.distributed.command.observations');
		}
		const key = expectationKey(observation);
		if (!expectations.has(key)) {
			protocolInvalid('extensions.distributed.command.observations');
		}
		observed.add(key);
	}
	return {
		causationId: command.causationId,
		expectations,
		observed
	};
}

function cloneOptimisticReceipt(
	receipt: OptimisticReceiptState
): OptimisticReceiptState {
	return {
		causationId: receipt.causationId,
		expectations: new Map(receipt.expectations),
		observed: new Set(receipt.observed)
	};
}

function sameReceipt(
	left: OptimisticReceiptState,
	right: OptimisticReceiptState
): boolean {
	return (
		left.causationId === right.causationId &&
		left.expectations.size === right.expectations.size &&
		[...left.expectations.keys()].every((key) =>
			right.expectations.has(key)
		)
	);
}

function expectationKey(value: {
	readonly projection: string;
	readonly model: string;
	readonly scopeToken: DistributedOpaqueString;
}): string {
	return JSON.stringify([value.projection, value.model, value.scopeToken]);
}

function incrementCanonicalDecimal(value: string): string {
	const digits = [...value];
	let carry = true;
	for (let index = digits.length - 1; index >= 0 && carry; index -= 1) {
		const digit = digits[index]!;
		if (digit === '9') {
			digits[index] = '0';
		} else {
			digits[index] = '0123456789'[
				'0123456789'.indexOf(digit) + 1
			]!;
			carry = false;
		}
	}
	if (carry) digits.unshift('1');
	return digits.join('');
}

function protocolInvalid(path: string): never {
	throw new DistributedProtocolError(
		'DISTRIBUTED_PROTOCOL_INVALID',
		path
	);
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

function protocolOperationSource(
	source: ReplicaWriteSource
): OperationProtocolSource {
	return source === 'live' ? 'live' : 'query';
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
