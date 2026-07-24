import {
	CacheRevisionConflictError,
	createCacheEngine,
	type BaseCacheWriter,
	type CacheEngine,
	type CacheEngineSnapshot,
	type CacheIndexCoverage,
	type CacheIndexMetadata,
	type CacheValue,
	type DerivedIndexMutation,
	type DerivedIndexReconciler,
	type OptimisticCacheWriter,
	type OptimisticIndexWrite,
	type OptimisticLayerView,
	type OptimisticRecordWrite,
	type RecordLink
} from '../internal/cache-engine.js';
import type { GqlError, GraphqlVariables } from '../types.js';
import {
	compareDistributedDecimal,
	DistributedProtocolError,
	isDistributedTrustedPresetCodec,
	parseDistributedTrustedPresetInventory,
	parseGraphqlResponseExtensions,
	type DistributedCommandMetadata,
	type DistributedDecimalString,
	type DistributedIndexRevision,
	type DistributedLiveCursor,
	type DistributedOpaqueString,
	type DistributedProjectionObservation,
	type DistributedProtocolEnvelope,
	type DistributedQuerySnapshot,
	type DistributedRecordRevision,
	type DistributedTrustedPreset
} from '../protocol.js';
import {
	matchReplicaTrustedPresetInventory,
	type ReplicaTrustedPresetDescriptor
} from './commands.js';
import type {
	ReplicaDiagnosticEventInput,
	ReplicaDiagnosticLayerInput,
	ReplicaDiagnosticReceiptInput,
	ReplicaDiagnosticsSink
} from './diagnostics.js';
import {
	replicaCommandAuthority,
	replicaResultObservation,
	type ReplicaCommandAuthorityRegistration,
	type ReplicaCommandAuthoritySnapshot,
	type ReplicaCommandSurfaceContract,
	type ReplicaResultObservationRegistration
} from './command-runtime.js';
import {
	canonicalizeOperationVariables,
	canonicalVariables,
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
import { createReplicaRevalidationMatcher } from './revalidation.js';
import {
	validateReplicaOperationBinding as validatedArtifactBinding,
	type ValidatedReplicaOperationBinding
} from './operation-binding.js';
import {
	createReplicaIndexMaintenanceRegistry,
	formatReplicaIndexStaleReason,
	type ReplicaIndexMaintenanceSnapshot,
	type ReplicaIndexPlanRegistration,
	type ReplicaIndexSemanticChange,
	type ReplicaIndexSemanticLayer
} from './index-maintenance.js';
import {
	embeddedRecordKey,
	runtimeRoot,
	type RuntimeObjectBranch,
	type RuntimeObjectSelection,
	type RuntimeRootSelection
} from './selection.js';
import type {
	DistributedReplicaOptions,
	DistributedReplica as DistributedReplicaApi,
	ReplicaAuthoritativeScope,
	ReplicaBaseWriter,
	ReplicaDehydratedState,
	ReplicaIdentity,
	ReplicaIndexInspection,
	ReplicaIndexTarget,
	ReplicaLiveState,
	ReplicaModelArtifact,
	ReplicaOperationArtifact,
	ReplicaOptimisticWriter,
	ReplicaRecordInspection,
	ReplicaRecordPatch,
	ReplicaRevalidationPlan,
	ReplicaRevision,
	ReplicaResultEnvelope,
	ReplicaSnapshot,
	ReplicaStatus,
	ReplicaTransport,
	ReplicaValue,
	ReplicaWatch,
	ReplicaWriteSource,
	WatchReplicaOptions
} from './types.js';

type QueryState = {
	fetching: boolean;
	errors: readonly GqlError[];
	live: ReplicaLiveState;
};

type LiveEntry = {
	count: number;
	unsubscribe: () => void;
	active: boolean;
	protocolGeneration: number;
	operationGeneration?: number;
};

type ProtocolGeneration = {
	protocolVersion: 2;
	cacheScope: DistributedOpaqueString;
	schemaHash: string;
};

type RenderedOperation = {
	readonly artifact: ReplicaOperationArtifact<unknown, GraphqlVariables>;
	readonly variables: GraphqlVariables;
};

type SerializedOperationProtocolState = {
	readonly operation: string;
	readonly snapshotScope?: string;
	readonly indexClocks: readonly (readonly [
		string,
		Readonly<{ scopeToken: string; position: string }>
	])[];
	readonly indexRevision?: string;
	readonly indexKeys: readonly string[];
	readonly pathRecords: readonly (readonly [string, string])[];
	readonly cursors: readonly DistributedLiveCursor[];
};

type SerializedOperationProtocolGroup = {
	readonly key: string;
	readonly query?: SerializedOperationProtocolState;
	readonly live?: SerializedOperationProtocolState;
	readonly active?: OperationProtocolSource;
	readonly generation: number;
};

type ReplicaDehydratedPayloadV1 = {
	readonly cache: CacheEngineSnapshot;
	readonly operations: readonly SerializedOperationProtocolGroup[];
	readonly recordClocks: readonly (readonly [string, RecordProtocolClock])[];
	readonly anonymousRecordClocks: readonly (readonly [
		string,
		AnonymousRecordProtocolClock
	])[];
	readonly trustedPresets: readonly DistributedTrustedPreset[];
	readonly nextIndexRevision: string;
};

type ParsedReplicaHydration = {
	readonly scope: ProtocolGeneration;
	readonly cache: CacheEngineSnapshot;
	readonly operationProtocols: Map<string, OperationProtocolGroup>;
	readonly operationGenerations: Map<string, number>;
	readonly recordClocks: Map<string, RecordProtocolClock>;
	readonly recordKeysByScope: Map<DistributedOpaqueString, string>;
	readonly anonymousRecordClocks: Map<
		DistributedOpaqueString,
		AnonymousRecordProtocolClock
	>;
	readonly trustedPresets: readonly DistributedTrustedPreset[];
	readonly nextIndexRevision: string;
};

type RegisteredCommandAuthorityContract = {
	readonly schemaHash: string;
	readonly protocolHash: string;
	readonly surfaceIdentity: string;
	readonly trustedPresets: readonly ReplicaTrustedPresetDescriptor[];
	readonly fingerprint: string;
};

type ReplicaArtifactBinding = {
	version: 2;
	schemaHash: string;
	surfaceIdentity?: string;
	trustedPresets?: readonly ReplicaTrustedPresetDescriptor[];
};

type ValidatedArtifactBinding = ValidatedReplicaOperationBinding;

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

type SharedIndexDisposition = {
	readonly compared: boolean;
	readonly disposition?: 'equal' | 'higher' | 'lower';
	readonly indexRevision?: string;
};

const EMPTY_ERRORS: readonly GqlError[] = Object.freeze([]);
/** Matches protocol.ts MAX_EVIDENCE_ITEMS without making it public API. */
const MAX_ANONYMOUS_RECORD_CLOCKS = 4_096;
const SHA256 = /^sha256:[0-9a-f]{64}$/;
const EMPTY_TRUSTED_PRESETS: readonly DistributedTrustedPreset[] = Object.freeze([]);
const EMPTY_CACHE_SNAPSHOT = Object.freeze({
	version: 1 as const,
	records: Object.freeze([]),
	indexes: Object.freeze([])
});

class DistributedReplicaImpl implements DistributedReplicaApi {
	readonly #engine: CacheEngine;
	readonly #transport: ReplicaTransport | undefined;
	readonly #reportObserverError: (error: AggregateError) => void;
	readonly #diagnostics: ReplicaDiagnosticsSink | undefined;
	readonly #diagnosticLayers:
		| Map<string, ReplicaDiagnosticLayerInput>
		| undefined;
	readonly #inFlight = new Map<string, Promise<void>>();
	readonly #inFlightAborts = new Map<string, AbortController>();
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
	readonly #renderedOperations = new Map<string, RenderedOperation>();
	readonly #readOperationKeys = new Set<string>();
	readonly #watchRenderCounts = new Map<string, number>();
	readonly #resultObservers = new Set<
		(envelope: ReplicaResultEnvelope<unknown>) => void
	>();
	readonly #indexMaintenance = createReplicaIndexMaintenanceRegistry();
	readonly #indexPlanRegistrations = new Map<
		string,
		ReplicaIndexPlanRegistration
	>();
	readonly #derivedIndexReconciler: DerivedIndexReconciler = (
		confirmed,
		layers
	) => this.#deriveMaintainedIndexes(confirmed, layers);
	#commandAuthorityContract: RegisteredCommandAuthorityContract | undefined;
	#trustedPresets: readonly DistributedTrustedPreset[] = EMPTY_TRUSTED_PRESETS;
	#authorizationAbort = new AbortController();
	#artifactBinding: ReplicaArtifactBinding | undefined;
	#protocolGeneration: ProtocolGeneration | undefined;
	#protocolGenerationSequence = 0;
	#nextIndexRevision = '0';
	#diagnosticLayerSequence = 0;

	constructor(options: DistributedReplicaOptions = {}) {
		this.#transport = options.transport;
		this.#reportObserverError = options.onObserverError ?? reportUnhandledObserverError;
		this.#diagnostics = options.diagnostics;
		this.#diagnosticLayers =
			options.diagnostics === undefined ? undefined : new Map();
		this.#engine = createCacheEngine({ onWatcherError: this.#reportObserverError });
		this.#engine.setDerivedIndexReconciler(this.#derivedIndexReconciler);
		this.#syncDiagnostics();
	}

	get scope(): ReplicaAuthoritativeScope | undefined {
		const scope = this.#protocolGeneration;
		return scope === undefined
			? undefined
			: Object.freeze({
					protocolVersion: scope.protocolVersion,
					schemaHash: scope.schemaHash,
					cacheScope: scope.cacheScope
				});
	}

	get authorizationGeneration(): number {
		return this.#protocolGenerationSequence;
	}

	[replicaCommandAuthority](
		contract: ReplicaCommandSurfaceContract
	): ReplicaCommandAuthorityRegistration {
		const next = validatedCommandAuthorityContract(contract);
		const current = this.#commandAuthorityContract;
		if (current !== undefined && current.fingerprint !== next.fingerprint) {
			throw new TypeError(
				'command inventory does not match the active replica client surface'
			);
		}

		const binding = this.#artifactBinding;
		if (binding === undefined) {
			this.#artifactBinding = Object.freeze({
				version: 2,
				schemaHash: next.schemaHash,
				surfaceIdentity: next.surfaceIdentity,
				trustedPresets: next.trustedPresets
			});
		} else if (
			binding.version !== 2 ||
			binding.schemaHash !== next.schemaHash ||
			(
				binding.surfaceIdentity !== undefined &&
				binding.surfaceIdentity !== next.surfaceIdentity
			) ||
			(
				binding.trustedPresets !== undefined &&
				trustedPresetDescriptorFingerprint(binding.trustedPresets) !==
					trustedPresetDescriptorFingerprint(next.trustedPresets)
			)
		) {
			throw new TypeError(
				'command inventory does not match the active replica client surface'
			);
		} else if (
			binding.surfaceIdentity === undefined ||
			binding.trustedPresets === undefined
		) {
			this.#artifactBinding = Object.freeze({
				...binding,
				...(binding.surfaceIdentity === undefined
					? { surfaceIdentity: next.surfaceIdentity }
					: {}),
				...(binding.trustedPresets === undefined
					? { trustedPresets: next.trustedPresets }
					: {})
			});
		}

		if (this.#protocolGeneration !== undefined) {
			try {
				matchReplicaTrustedPresetInventory(
					next.trustedPresets,
					this.#trustedPresets
				);
			} catch (error) {
				this.#purgeProtocolGeneration();
				throw error;
			}
		}
		this.#commandAuthorityContract = next;

		let active = true;
		const read = (): ReplicaCommandAuthoritySnapshot => {
			if (!active) {
				throw new TypeError('replica command authority registration is disposed');
			}
			return Object.freeze({
				generation: this.#protocolGenerationSequence,
				scope: this.scope,
				trustedPresets: this.#trustedPresets,
				signal: this.#authorizationAbort.signal
			});
		};
		return Object.freeze({
			read,
			dispose(): void {
				active = false;
			}
		});
	}

	[replicaResultObservation](
		observer: (envelope: ReplicaResultEnvelope<unknown>) => void
	): ReplicaResultObservationRegistration {
		if (typeof observer !== 'function') {
			throw new TypeError('replica result observer must be a function');
		}
		this.#resultObservers.add(observer);
		let active = true;
		return Object.freeze({
			dispose: (): void => {
				if (!active) return;
				active = false;
				this.#resultObservers.delete(observer);
			}
		});
	}

	read<TData, TVariables extends GraphqlVariables>(
		artifact: ReplicaOperationArtifact<TData, TVariables>,
		variables: TVariables
	): ReplicaSnapshot<TData> {
		this.#bindArtifact(artifact);
		const stableVariables = canonicalizeOperationVariables(artifact, variables);
		const key = operationKey(artifact, stableVariables);
		this.#rememberRenderedOperation(key, artifact, stableVariables, 'read');
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
		this.#bindArtifact(artifact);
		return new ReplicaWatchState(this, artifact, variables, options);
	}

	writeResult<TData, TVariables extends GraphqlVariables>(
		artifact: ReplicaOperationArtifact<TData, TVariables>,
		variables: TVariables,
		envelope: ReplicaResultEnvelope<TData>,
		source: ReplicaWriteSource
	): void {
		assertWriteSource(source);
		this.#bindArtifact(artifact);
		const stableVariables = canonicalizeOperationVariables(artifact, variables);
		this.#writeCanonicalResult(artifact, stableVariables, envelope, source);
	}

	revalidate(plan: ReplicaRevalidationPlan): Promise<void> {
		const matches = createReplicaRevalidationMatcher(plan);
		const generation = this.#protocolGenerationSequence;
		const active = new Map<
			string,
			ReplicaWatchState<unknown, GraphqlVariables>
		>();
		for (const [key, watches] of this.#watches) {
			const watch = watches.values().next().value;
			if (watch !== undefined && matches(watch.artifact)) {
				active.set(key, watch);
			}
		}
		/*
		 * A command-status observation may arrive while an older query is
		 * already in flight. Drain that request first; the authoritative
		 * revalidation below must start after the command fence.
		 */
		const prior = [...active.keys()].flatMap((key) => {
			const request = this.#inFlight.get(key);
			return request === undefined ? [] : [request];
		});
		return Promise.all(prior)
			.then(() => {
				if (this.#protocolGenerationSequence !== generation) return [];

				/*
				 * Stale every rendered matching root before fetching. This both
				 * starts active watches through their normal coordinator and
				 * leaves inactive SSR/read consumers fail-closed until a future
				 * watch can refresh them.
				 */
				const rootKeys = new Set<string>();
				for (const rendered of this.#renderedOperations.values()) {
					if (!matches(rendered.artifact)) continue;
					for (const root of rendered.artifact.roots) {
						rootKeys.add(
							replicaIndexKey({
								field: root.field,
								arguments: resolveArguments(
									root.arguments,
									rendered.variables,
									root.coverage
								)
							})
						);
					}
				}
				for (const key of rootKeys) {
					this.#engine.batch((writer) =>
						writer.markIndexStale(
							key,
							'command-authoritative-revalidation'
						)
					);
				}

				// One operation key may have several component subscribers. The
				// existing fetch coordinator owns deduplication and response fences.
				return [...active.values()].map((watch) =>
					this._fetch(watch, true).then(() => watch)
				);
			})
			.then((requests) => Promise.all(requests))
			.then((watches) => {
				if (this.#protocolGenerationSequence !== generation) return;
				const failed = watches.some((watch) => {
					const state = this.#queryState(watch.key);
					/*
					 * Revalidation proves that the authoritative HTTP response
					 * refreshed the confirmed graph. A surviving accepted layer
					 * may deliberately keep the visible index stale when its row
					 * policy cannot be evaluated locally; the command runtime
					 * retires that layer only after this proof succeeds.
					 */
					const confirmed = this.#engine.readConfirmed((reader) =>
						materializeReplicaOperation(
							reader,
							watch.artifact,
							watch.variables
						)
					);
					return (
						state.errors.length > 0 ||
						!confirmed.complete ||
						confirmed.stale
					);
				});
				if (failed) {
					throw new Error(
						'authoritative command revalidation did not produce a complete result'
					);
				}
			});
	}

	invalidateAuthorization(): void {
		this.#purgeProtocolGeneration();
	}

	dehydrate(): ReplicaDehydratedState {
		const scope = this.#protocolGeneration;
		if (scope === undefined) {
			throw new Error(
				'cannot dehydrate replica before the server establishes an authoritative scope'
			);
		}
		const reachable = this.#reachableConfirmedState();
		const reachableIndexKeys = new Set(
			reachable.cache.indexes.map((index) => index.key)
		);
		const operationKeys = new Set(this.#renderedOperations.keys());
		for (const [key, group] of this.#operationProtocols) {
			const governsReachableState = [group.query, group.live].some(
				(state) =>
					state !== undefined &&
					(
						[...state.indexKeys].some((indexKey) =>
							reachableIndexKeys.has(indexKey)
						) ||
						[...state.pathRecords.values()].some((recordKey) =>
							reachable.clockRecordKeys.has(recordKey)
						)
					)
			);
			if (governsReachableState) operationKeys.add(key);
		}
		const operations = [...operationKeys]
			.sort()
			.flatMap((key) => {
				const group = this.#operationProtocols.get(key);
				if (group === undefined) return [];
				return [
					serializeOperationProtocolGroup(
						key,
						group,
						this.#operationGeneration(key)
					)
				];
			});
		const recordClocks = [...this.#recordClocks]
			.filter(([key]) => reachable.clockRecordKeys.has(key))
			.sort(([left], [right]) => left.localeCompare(right))
			.map(([key, clock]) =>
				Object.freeze([key, freezeRecordClock(clock)] as const)
			);
		const anonymousRecordClocks = [...this.#anonymousRecordClocks]
			.filter(([, value]) => reachable.models.has(value.model))
			.sort(([left], [right]) => left.localeCompare(right))
			.map(([scopeToken, value]) =>
				Object.freeze([
					scopeToken,
					Object.freeze({
						model: value.model,
						clock: freezeRecordClock(value.clock)
					})
				] as const)
			);
			const payload: ReplicaDehydratedPayloadV1 = Object.freeze({
				cache: reachable.cache,
				operations: Object.freeze(operations),
				recordClocks: Object.freeze(recordClocks),
				anonymousRecordClocks: Object.freeze(anonymousRecordClocks),
				trustedPresets: this.#trustedPresets,
				nextIndexRevision: this.#nextIndexRevision
			});
		const state = Object.freeze({
			version: 1 as const,
			scope: Object.freeze({
				protocolVersion: 2 as const,
				schemaHash: scope.schemaHash,
				cacheScope: scope.cacheScope
			}),
			payload
		});
		this.#finishDehydration();
		return state;
	}

	hydrate(
		state: ReplicaDehydratedState,
		authoritativeScope: ReplicaAuthoritativeScope
	): boolean {
		const rejected = (
			reason:
				| 'invalid'
				| 'scope-mismatch'
				| 'artifact-mismatch'
				| 'active-scope-mismatch'
				| 'metadata-mismatch'
		): false => {
			if (this.#diagnostics !== undefined) {
				this.#diagnosticEvent(
					Object.freeze({ kind: 'hydration', action: 'rejected', reason })
				);
			}
			return false;
		};
		const parsed = parseReplicaHydration(state);
		const expectedScope = parseAuthoritativeScope(authoritativeScope);
		if (parsed === undefined || expectedScope === undefined) {
			return rejected('invalid');
		}
		if (
			parsed.scope.protocolVersion !== expectedScope.protocolVersion ||
			parsed.scope.schemaHash !== expectedScope.schemaHash ||
			parsed.scope.cacheScope !== expectedScope.cacheScope
		) {
			return rejected('scope-mismatch');
		}
		const binding = this.#artifactBinding;
		if (
			binding !== undefined &&
				binding.schemaHash !== parsed.scope.schemaHash
		) {
			return rejected('artifact-mismatch');
		}
		const current = this.#protocolGeneration;
		if (
			current !== undefined &&
			(
				current.protocolVersion !== parsed.scope.protocolVersion ||
				current.schemaHash !== parsed.scope.schemaHash ||
				current.cacheScope !== parsed.scope.cacheScope
			)
		) {
			return rejected('active-scope-mismatch');
		}
		const preserveLocalCommandState = current !== undefined;

		// Validate the private engine payload before closing transports or changing
		// any live state. `restore` parses fully before its own transaction.
		try {
			createCacheEngine().restore(parsed.cache);
			const expectedTrustedPresets =
				binding?.trustedPresets !== undefined
					? binding.trustedPresets
					: this.#commandAuthorityContract?.trustedPresets;
			if (expectedTrustedPresets !== undefined) {
				matchReplicaTrustedPresetInventory(
					expectedTrustedPresets,
					parsed.trustedPresets
				);
			}
			if (
				preserveLocalCommandState &&
				trustedPresetInventoryFingerprint(this.#trustedPresets) !==
					trustedPresetInventoryFingerprint(parsed.trustedPresets)
			) {
				throw new TypeError(
					'trusted presets changed within one authoritative cache scope'
				);
			}
		} catch {
			return rejected('invalid');
		}
		if (!hydrationMetadataConsistent(parsed)) {
			return rejected('metadata-mismatch');
		}

		if (preserveLocalCommandState) {
			this.#closeActiveTransports();
		} else {
			this.#closeAuthorizationGeneration();
		}
		this.#queryStates.clear();
		this.#operationProtocols.clear();
		for (const [key, group] of parsed.operationProtocols) {
			this.#operationProtocols.set(key, group);
		}
		this.#operationGenerations.clear();
		for (const [key, generation] of parsed.operationGenerations) {
			this.#operationGenerations.set(key, generation);
		}
		this.#recordClocks.clear();
		for (const [key, clock] of parsed.recordClocks) {
			this.#recordClocks.set(key, clock);
		}
		this.#recordKeysByScope.clear();
		for (const [scopeToken, key] of parsed.recordKeysByScope) {
			this.#recordKeysByScope.set(scopeToken, key);
		}
		this.#anonymousRecordClocks.clear();
		for (const [scopeToken, clock] of parsed.anonymousRecordClocks) {
			this.#anonymousRecordClocks.set(scopeToken, clock);
		}
		if (!preserveLocalCommandState) {
			this.#optimisticReceipts.clear();
			this.#diagnosticLayers?.clear();
			this.#diagnosticLayerSequence = 0;
		}
		this.#trustedPresets = parsed.trustedPresets;
		this.#nextIndexRevision = parsed.nextIndexRevision;
		this.#protocolGeneration = parsed.scope;
		this.#artifactBinding = Object.freeze({
			version: 2,
			schemaHash: parsed.scope.schemaHash,
			...(binding?.surfaceIdentity !== undefined
				? { surfaceIdentity: binding.surfaceIdentity }
				: {}),
			...(binding?.trustedPresets !== undefined
				? { trustedPresets: binding.trustedPresets }
				: {})
		});
		if (preserveLocalCommandState) {
			this.#engine.restoreConfirmed(parsed.cache);
		} else {
			this.#engine.restore(parsed.cache);
		}
		this.#resumeLiveWatches();
		this.#syncDiagnostics();
		if (this.#diagnostics !== undefined) {
			this.#diagnosticEvent(
				Object.freeze({
					kind: 'hydration',
					action: 'accepted',
					reason: 'accepted'
				})
			);
			this.#diagnosticEvent(
				Object.freeze({
					kind: 'scope',
					action: 'established',
					generation: this.#protocolGenerationSequence,
					schemaHash: parsed.scope.schemaHash
				})
			);
		}
		return true;
	}

	#reachableConfirmedState(): {
		readonly cache: CacheEngineSnapshot;
		readonly recordKeys: ReadonlySet<string>;
		readonly clockRecordKeys: ReadonlySet<string>;
		readonly models: ReadonlySet<string>;
	} {
		const snapshot = this.#engine.extract();
		const indexes = new Map(snapshot.indexes.map((index) => [index.key, index]));
		const records = new Map(snapshot.records.map((record) => [record.key, record]));
		const indexKeys = new Set<string>();
		const recordKeys = new Set<string>();
		const clockRecordKeys = new Set<string>();
		const recordFields = new Map<string, Set<string>>();
		const models = new Set<string>();

		const rememberSelection = (selection: RuntimeObjectSelection): void => {
			if (selection.storage.kind === 'normalized') {
				models.add(selection.storage.model);
			}
			for (const member of selection.members) {
				if (member.kind === 'branch') rememberSelection(member.selection);
			}
		};
		const rememberRecord = (
			recordKey: string,
			selection: RuntimeObjectSelection
		): void => {
			recordKeys.add(recordKey);
			clockRecordKeys.add(recordKey);
			let selected = recordFields.get(recordKey);
			if (selected === undefined) {
				selected = new Set();
				recordFields.set(recordKey, selected);
			}
			for (const member of selection.members) {
				if (member.kind === 'scalar') selected.add(member.field);
			}
		};
		const visitObject = (
			selection: RuntimeObjectSelection,
			recordKey: string,
			variables: GraphqlVariables
		): void => {
			rememberRecord(recordKey, selection);
			for (const member of selection.members) {
				if (member.kind !== 'branch') continue;
				visitBranch(member, recordKey, variables);
			}
		};
		const visitBranch = (
			selection: RuntimeRootSelection | RuntimeObjectBranch,
			parent: string | undefined,
			variables: GraphqlVariables
		): void => {
			const key = replicaIndexKey({
				...(parent === undefined ? {} : { parent }),
				field: selection.field,
				arguments: resolveArguments(
					selection.arguments,
					variables,
					selection.coverage
				)
			});
			const index = indexes.get(key);
			if (index === undefined) return;
			indexKeys.add(key);
			for (const recordKey of index.records) {
				visitObject(selection.selection, recordKey, variables);
			}
		};

		for (const [key, rendered] of this.#renderedOperations) {
			for (const rootArtifact of rendered.artifact.roots) {
				const root = runtimeRoot(rootArtifact);
				rememberSelection(root.selection);
				visitBranch(root, undefined, rendered.variables);
			}
			const group = this.#operationProtocols.get(key);
			for (const state of [group?.query, group?.live]) {
				for (const recordKey of state?.pathRecords.values() ?? []) {
					clockRecordKeys.add(recordKey);
				}
			}
		}

		return Object.freeze({
			cache: Object.freeze({
				version: 1 as const,
				records: Object.freeze(
					[...recordKeys]
						.sort()
						.flatMap((key) => {
							const record = records.get(key);
							if (record === undefined) return [];
							const selected = recordFields.get(key) ?? new Set();
							return [
								Object.freeze({
									...record,
									fields: Object.freeze(
										Object.fromEntries(
											Object.entries(record.fields).filter(([field]) =>
												selected.has(field)
											)
										)
									),
									links: Object.freeze({})
								})
							];
						})
				),
				indexes: Object.freeze(
					[...indexKeys]
						.sort()
						.flatMap((key) => {
							const index = indexes.get(key);
							return index === undefined ? [] : [index];
						})
				)
			}),
			recordKeys,
			clockRecordKeys,
			models
		});
	}

	#bindArtifact<TData, TVariables extends GraphqlVariables>(
		artifact: ReplicaOperationArtifact<TData, TVariables>
	): void {
		const next = validatedArtifactBinding(artifact);
		this.#validateActiveArtifactTrustedPresets(next);
		const current = this.#artifactBinding;
		if (current === undefined) {
			this.#artifactBinding = Object.freeze({
				version: next.version,
				schemaHash: next.schemaHash,
				surfaceIdentity: next.surfaceIdentity,
				trustedPresets: next.trustedPresets
			});
			return;
		}
		if (
			(
				current.surfaceIdentity === undefined ||
				current.trustedPresets === undefined
			) &&
			current.version === next.version &&
			current.schemaHash === next.schemaHash &&
			(
				current.surfaceIdentity === undefined ||
				current.surfaceIdentity === next.surfaceIdentity
			) &&
			(
				current.trustedPresets === undefined ||
				trustedPresetDescriptorFingerprint(current.trustedPresets) ===
					trustedPresetDescriptorFingerprint(next.trustedPresets)
			)
		) {
			this.#artifactBinding = Object.freeze({
				...current,
				surfaceIdentity: next.surfaceIdentity,
				trustedPresets: next.trustedPresets
			});
			return;
		}
		if (
			current.version !== next.version ||
			current.schemaHash !== next.schemaHash ||
			(
				current.surfaceIdentity !== undefined &&
				current.surfaceIdentity !== next.surfaceIdentity
			) ||
			(
				current.trustedPresets !== undefined &&
				trustedPresetDescriptorFingerprint(
					current.trustedPresets
				) !== trustedPresetDescriptorFingerprint(next.trustedPresets)
			)
		) {
			throw new TypeError(
				'replica artifact schema does not match the active replica binding'
			);
		}
	}

	#validateActiveArtifactTrustedPresets(
		binding: ValidatedArtifactBinding
	): void {
		if (this.#protocolGeneration === undefined) {
			return;
		}
		try {
			matchReplicaTrustedPresetInventory(
				binding.trustedPresets,
				this.#trustedPresets
			);
		} catch (error) {
			this.#purgeProtocolGeneration();
			throw error;
		}
	}

	#writeCanonicalResult<TData, TVariables extends GraphqlVariables>(
		artifact: ReplicaOperationArtifact<TData, TVariables>,
		stableVariables: TVariables,
		envelope: ReplicaResultEnvelope<TData>,
		source: ReplicaWriteSource,
		requestRevision?: string
	): DistributedProtocolEnvelope {
		const extensions = parseGraphqlResponseExtensions(envelope.extensions);
		const parsedEnvelope: ReplicaResultEnvelope<TData> = Object.freeze({
			...envelope,
			...(extensions === undefined ? {} : { extensions })
		});
		const key = operationKey(artifact, stableVariables);
		const distributed = extensions?.distributed;
		if (distributed === undefined) {
			protocolInvalid('extensions.distributed');
		}
		const accepted = this.#writeProtocolResult(
			key,
			artifact,
			stableVariables,
			parsedEnvelope,
			source,
			distributed,
			requestRevision
		);
		if (accepted) this.#notifyResultObservers(parsedEnvelope);
		return distributed;
	}

	#writeProtocolResult<TData, TVariables extends GraphqlVariables>(
		key: string,
		artifact: ReplicaOperationArtifact<TData, TVariables>,
		stableVariables: TVariables,
		envelope: ReplicaResultEnvelope<TData>,
		source: ReplicaWriteSource,
		distributed: DistributedProtocolEnvelope,
		requestRevision: string | undefined
	): boolean {
			this.#validateProtocolBinding(artifact, distributed, source);
			const previousProtocolGeneration = this.#protocolGeneration;
			const nextProtocolGeneration =
				this.#stageProtocolGeneration(distributed);
			const nextTrustedPresets = this.#stageTrustedPresets(
				distributed.trustedPresets,
				nextProtocolGeneration,
				artifact
			);

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
				this.#trustedPresets = nextTrustedPresets;
				this.#protocolGeneration = nextProtocolGeneration;
				this.#resumeLiveWatches();
				this.#diagnosticScopeTransition(
					previousProtocolGeneration,
					nextProtocolGeneration
				);
				this.#syncDiagnostics();
				return true;
		}

		this.#validateLiveSnapshot(snapshot, live);
		const unsupportedLive =
			source === 'live' && live?.supported === false;
		const reset = live?.reset === true;
		const group = this.#operationProtocols.get(key)!;
		/*
		 * An unsupported subscription response is an authorized fallback
		 * snapshot, not a live source. In particular, a row-filtered snapshot
		 * has no comparable index vector, so retaining live ownership here
		 * would reject every later query handoff. Relinquish any prior live
		 * ownership without advancing the generation; the forced HTTP fallback
		 * starts against the generation that remains after this frame.
		 */
		if (unsupportedLive && group.active === 'live') {
			group.active = undefined;
		}
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
		const sharedDisposition = this.#sharedIndexDisposition(
			key,
			replicaResultIndexKeys(
				artifact,
				stableVariables,
				envelope,
				snapshot
			),
			snapshot,
			requestRevision,
			source
		);
		const handoffBlocked =
			handoff &&
			(
				!snapshot.indexesComparable ||
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
		if (
			!handoffBlocked &&
			isComparableHandoffDisposition(disposition) &&
			sharedDisposition.compared
		) {
			disposition =
				sharedDisposition.disposition === 'lower'
					? 'lower'
					: sharedDisposition.disposition ?? disposition;
		}
		const sourceSwitched =
			!unsupportedLive &&
			!handoffBlocked &&
			isComparableHandoffDisposition(disposition) &&
			this.#activateOperationSource(
				key,
				operationSource,
				artifact,
				stableVariables
			);
		const rejectedHandoff = handoff && !sourceSwitched;
		if (
			snapshot.indexesComparable &&
			(reset || ownDisposition === 'incomparable')
		) {
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

		if (!snapshot.indexesComparable) {
			if (rejectedHandoff) {
				this.#resetOperationState(operationState);
			} else {
				/*
				 * The server-authorized GraphQL payload is still an exact
				 * replacement result. Only its partition-wide causal vector is
				 * unavailable (most commonly because exposing that position
				 * would leak denied-row activity). Preserve local membership,
				 * while dropping every capability that requires comparison.
				 */
				this.#resetOperationCausalState(operationState);
			}
			disposition = rejectedHandoff
				? 'incomparable'
				: sharedDisposition.disposition === 'lower'
					? 'lower'
					: 'fresh';
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
			!rejectedHandoff &&
			disposition !== 'lower' &&
			disposition !== 'incomparable';
		/*
		 * Revision zero is the cache engine's lowest legal checkpoint. It lets
		 * an unsupported live response fill an empty cache immediately while
		 * guaranteeing that any HTTP request revision can replace it.
		 */
		const indexRevision =
			unsupportedLive
				? '0'
				: writeIndexes &&
					  (
							sourceSwitched ||
							sharedDisposition.disposition === 'higher'
						)
					? this.#allocateIndexRevision()
					: writeIndexes &&
						  sharedDisposition.disposition === 'equal' &&
						  sharedDisposition.indexRevision !== undefined
						? sharedDisposition.indexRevision
				: snapshot.indexesComparable &&
					  disposition === 'equal' &&
					  operationState.indexRevision !== undefined
					? operationState.indexRevision
					: (requestRevision ?? this.#allocateIndexRevision());
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
			snapshot.recordsComplete &&
				snapshot.indexesComparable &&
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
			/*
			 * This is cache-membership completeness, not causal-vector
			 * comparability. normalizeReplicaResult independently downgrades
			 * GraphQL path errors and missing selected fields.
			 */
			indexesComplete: true,
			allowSnapshotOnlyRecords: !snapshot.recordsComplete,
			record: (
				path,
				model,
				recordKey
			): ReplicaProtocolRecordResolution | undefined => {
				const encodedPath = responsePathKey(path);
				const evidence = recordEvidence.byPath.get(encodedPath);
				if (evidence === undefined) {
					if (snapshot.recordsComplete) {
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
				this.#retireDiagnosticLayer(id, 'retired', 'projected', receipt);
				this.#optimisticReceipts.delete(id);
			} else {
				this.#optimisticReceipts.set(id, receipt);
				this.#engine.markOptimisticLayerAccepted(id);
				const layer = this.#diagnosticLayers?.get(id);
				if (layer !== undefined) {
					this.#diagnosticLayers!.set(
						id,
						Object.freeze({ ...layer, state: 'accepted' as const })
					);
				}
				if (this.#diagnostics !== undefined) {
					const counts = diagnosticReceiptCounts(receipt);
					this.#diagnosticEvent(
						Object.freeze({
							kind: 'receipt',
							command: id,
							state:
								counts.obligations === 0
									? ('accepted' as const)
									: ('accepted_pending_projection' as const),
							obligations: counts.obligations,
							observed: counts.observed
						})
					);
				}
			}
		}
		if (writeIndexes) {
			operationState.indexRevision = indexRevision;
			for (const indexKey of summary.indexKeys) {
				operationState.indexKeys.add(indexKey);
			}
			if (snapshot.indexesComparable) {
				operationState.snapshotScope = snapshot.scopeToken;
				operationState.indexClocks = indexClockMap(snapshot.indexes);
				operationState.cursors = latestCursors(snapshot, live);
			} else {
				this.#resetOperationCausalState(operationState);
				operationState.indexRevision = indexRevision;
				for (const indexKey of summary.indexKeys) {
					operationState.indexKeys.add(indexKey);
				}
			}
		} else if (live?.reset === true || !snapshot.indexesComparable) {
			operationState.cursors = Object.freeze([]);
		}
		if (source === 'live' && !unsupportedLive) {
			this.#advanceOperationGeneration(key);
		}
			if (source !== 'live' && sourceSwitched) {
				this.#restartLive(key);
			}
			this.#trustedPresets = nextTrustedPresets;
			this.#protocolGeneration = nextProtocolGeneration;
		this.#resumeLiveWatches();
		this.#emitState(key, false);
		if (this.#diagnostics !== undefined) {
			const cache = this.#engine.extract();
			this.#diagnosticEvent(
				Object.freeze({
					kind: 'normalization',
					operation: artifact.id,
					source,
					records: cache.records.length,
					indexes: summary.indexKeys.length,
					partial:
						summary.partial ||
						disposition === 'lower' ||
						disposition === 'incomparable' ||
						rejectedHandoff
				})
			);
			for (const indexKey of summary.indexKeys) {
				const index = cache.indexes.find(
					(candidate) => candidate.key === indexKey && !candidate.deleted
				);
				const staleReason = index?.metadata?.staleReason;
				this.#diagnosticEvent(
					Object.freeze({
						kind: 'index-decision',
						index: indexKey,
						decision:
							!writeIndexes ||
							!snapshot.indexesComparable ||
							disposition === 'incomparable'
								? ('revalidate' as const)
								: staleReason === undefined
									? ('maintained' as const)
									: ('stale' as const),
						...(staleReason === undefined
							? {}
							: { reason: staleReason })
					})
				);
			}
			this.#diagnosticScopeTransition(
				previousProtocolGeneration,
				nextProtocolGeneration
			);
			this.#syncDiagnostics();
		}
		if (disposition === 'incomparable' || rejectedHandoff) return false;
		if (disposition !== 'lower') return true;
		// A lower index snapshot from the already-active source cannot replace
		// membership, but it can still carry independently fenced record clocks
		// and exact causal observations. Notify only when that transaction
		// actually committed data or receipt progress.
		return summary.wrote || receiptPlan.satisfied.length > 0;
	}

	#notifyResultObservers(envelope: ReplicaResultEnvelope<unknown>): void {
		if (this.#resultObservers.size === 0) return;
		const errors: unknown[] = [];
		for (const observer of [...this.#resultObservers]) {
			try {
				observer(envelope);
			} catch (error) {
				errors.push(error);
			}
		}
		this._reportObserverErrors(errors);
	}

	createOptimisticLayer(
		id: string,
		update: (writer: ReplicaOptimisticWriter) => void,
		semanticChanges: readonly ReplicaIndexSemanticChange[] = Object.freeze([])
	): void {
		assertReplicaOptimisticLayerId(id);
		if (this.#engine.optimisticLayerState(id) !== undefined) {
			throw new Error(`optimistic layer already exists: ${id}`);
		}
		if (typeof update !== 'function') {
			throw new TypeError('optimistic layer update must be a function');
		}
		const captured = captureReplicaOptimisticUpdate(
			id,
			update,
			semanticChanges
		);
		this.#engine.createOptimisticLayer(
			id,
			(writer) => replayReplicaOptimisticUpdate(writer, captured.operations),
			captured.context
		);
		if (this.#diagnosticLayers !== undefined) {
			const recordChanges = captured.operations.filter(
				(operation) =>
					operation.kind === 'write-record' ||
					operation.kind === 'tombstone-record'
			).length;
			const indexChanges = captured.operations.length - recordChanges;
			const layer = Object.freeze({
				id,
				sequence: ++this.#diagnosticLayerSequence,
				state: 'optimistic' as const,
				recordChanges,
				indexChanges,
				semanticChanges: recordChanges + semanticChanges.length
			});
			this.#diagnosticLayers.set(id, layer);
			this.#diagnosticEvent(
				Object.freeze({
					kind: 'layer',
					layer: id,
					action: 'created',
					recordChanges,
					indexChanges
				})
			);
			this.#diagnosticEvent(
				Object.freeze({
					kind: 'receipt',
					command: id,
					state: 'optimistic',
					obligations: 0,
					observed: 0
				})
			);
			this.#syncDiagnostics();
		}
	}

	markOptimisticLayerAccepted(
		id: string,
		receipt?: DistributedCommandMetadata
	): boolean {
		if (receipt !== undefined && receipt.commandId !== id) {
			throw new TypeError('optimistic layer id must equal the causal command id');
		}
		const accepted = this.#engine.markOptimisticLayerAccepted(id);
		if (!accepted) return false;
		const diagnosticLayer = this.#diagnosticLayers?.get(id);
		if (diagnosticLayer !== undefined) {
			this.#diagnosticLayers!.set(
				id,
				Object.freeze({ ...diagnosticLayer, state: 'accepted' as const })
			);
			this.#diagnosticEvent(
				Object.freeze({
					kind: 'layer',
					layer: id,
					action: 'accepted',
					recordChanges: diagnosticLayer.recordChanges,
					indexChanges: diagnosticLayer.indexChanges
				})
			);
		}
		if (receipt === undefined) {
			if (this.#diagnostics !== undefined) {
				this.#diagnosticEvent(
					Object.freeze({
						kind: 'receipt',
						command: id,
						state: 'accepted',
						obligations: 0,
						observed: 0
					})
				);
			}
			this.#syncDiagnostics();
			return true;
		}
		const next = optimisticReceiptState(receipt);
		const current = this.#optimisticReceipts.get(id);
		if (current !== undefined && !sameReceipt(current, next)) {
			throw new DistributedProtocolError(
				'DISTRIBUTED_PROTOCOL_INVALID',
				'extensions.distributed.command'
			);
		}
		this.#optimisticReceipts.set(id, next);
		if (this.#diagnostics !== undefined) {
			const counts = diagnosticReceiptCounts(next);
			this.#diagnosticEvent(
				Object.freeze({
					kind: 'receipt',
					command: id,
					state:
						counts.obligations === 0
							? ('accepted' as const)
							: ('accepted_pending_projection' as const),
					obligations: counts.obligations,
					observed: counts.observed
				})
			);
		}
		this.#syncDiagnostics();
		return true;
	}

	confirmOptimisticLayer<T>(
		id: string,
		update: (writer: ReplicaBaseWriter) => T
	): T {
		const result = this.#engine.confirmOptimisticLayer(id, (writer) =>
			update(baseWriter(writer))
		);
		this.#retireDiagnosticLayer(id, 'retired', 'projected');
		this.#optimisticReceipts.delete(id);
		this.#syncDiagnostics();
		return result;
	}

	rejectOptimisticLayer(id: string): boolean {
		const rejected = this.#engine.rejectOptimisticLayer(id);
		if (rejected) {
			this.#retireDiagnosticLayer(id, 'rejected', 'rejected');
			this.#optimisticReceipts.delete(id);
			this.#syncDiagnostics();
		}
		return rejected;
	}

	tombstoneRecord(
		model: ReplicaModelArtifact,
		identity: ReplicaIdentity,
		revision: ReplicaRevision
	): boolean {
		const wrote = this.#engine.batch((writer) =>
			writer.tombstoneRecord(replicaRecordKey(model, identity), revision)
		);
		if (wrote) this.#syncDiagnostics();
		return wrote;
	}

	markIndexStale(target: ReplicaIndexTarget, reason: string): boolean {
		const key = indexKeyFromTarget(target);
		const marked = this.#engine.batch((writer) =>
			writer.markIndexStale(key, reason)
		);
		if (marked) {
			if (this.#diagnostics !== undefined) {
				this.#diagnosticEvent(
					Object.freeze({
						kind: 'index-decision',
						index: key,
						decision: 'stale',
						reason
					})
				);
			}
			this.#syncDiagnostics();
		}
		return marked;
	}

	retainRecord(model: ReplicaModelArtifact, identity: ReplicaIdentity): void {
		this.#engine.retain(replicaRecordKey(model, identity));
	}

	releaseRecord(model: ReplicaModelArtifact, identity: ReplicaIdentity): void {
		this.#engine.release(replicaRecordKey(model, identity));
	}

	gc(): readonly string[] {
		const collected = this.#engine.gc();
		if (this.#diagnostics !== undefined) {
			this.#diagnosticEvent(
				Object.freeze({
					kind: 'gc',
					records: collected.length
				})
			);
		}
		this.#syncDiagnostics();
		return collected;
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

	#syncDiagnostics(): void {
		const diagnostics = this.#diagnostics;
		if (diagnostics === undefined) return;
		try {
			const cache = this.#engine.extract();
			const records = cache.records.map((record) => {
				const model = modelFromRecordKey(record.key);
				const tombstone = record.tombstoneRevision !== undefined;
				const values: Record<string, ReplicaValue> = {};
				if (!tombstone && diagnostics.redactRecordValue !== undefined) {
					for (const [field, entry] of Object.entries(record.fields)) {
						const value = diagnostics.redactRecordValue(
							Object.freeze({
								recordKey: record.key,
								...(model === undefined ? {} : { model }),
								field,
								kind: 'field' as const
							}),
							entry.value as ReplicaValue
						);
						if (value !== undefined) values[field] = value;
					}
					for (const [field, entry] of Object.entries(record.links)) {
						const value = diagnostics.redactRecordValue(
							Object.freeze({
								recordKey: record.key,
								...(model === undefined ? {} : { model }),
								field,
								kind: 'link' as const
							}),
							entry.value as ReplicaValue
						);
						if (value !== undefined) values[field] = value;
					}
				}
				return Object.freeze({
					key: record.key,
					...(model === undefined ? {} : { model }),
					revision: record.revision,
					incarnation: record.incarnation ?? record.revision,
					tombstone,
					...(record.tombstoneRevision === undefined
						? {}
						: { tombstoneRevision: record.tombstoneRevision }),
					presentFields: Object.freeze(
						tombstone ? [] : Object.keys(record.fields).sort()
					),
					presentLinks: Object.freeze(
						tombstone ? [] : Object.keys(record.links).sort()
					),
					...(Object.keys(values).length === 0
						? {}
						: { values: Object.freeze(values) })
				});
			});
			const indexes = cache.indexes.map((index) =>
				Object.freeze({
					key: index.key,
					revision: index.revision,
					...(index.staleRevision === undefined
						? {}
						: { staleRevision: index.staleRevision }),
					records: index.records,
					complete: index.complete,
					deleted: index.deleted,
					...(index.metadata === undefined
						? {}
						: {
								field: index.metadata.field,
								...(index.metadata.parent === undefined
									? {}
									: { parent: index.metadata.parent }),
								argumentNames: Object.freeze(
									Object.keys(index.metadata.arguments).sort()
								),
								...(diagnostics.includeStructuralIdentities
									? { arguments: index.metadata.arguments }
									: {}),
								coverage: index.metadata.coverage,
								dependencies: index.metadata.dependencies,
								...(index.metadata.staleReason === undefined
									? {}
									: { staleReason: index.metadata.staleReason }),
								nullValue: index.metadata.nullValue === true
							})
				})
			);
			const receipts: ReplicaDiagnosticReceiptInput[] = [];
			for (const layer of this.#diagnosticLayers?.values() ?? []) {
				const receipt = this.#optimisticReceipts.get(layer.id);
				receipts.push(
					Object.freeze({
						commandId: layer.id,
						state:
							receipt === undefined
								? ('optimistic' as const)
								: receipt.expectations.size === 0
									? ('accepted' as const)
									: ('accepted_pending_projection' as const),
						expectations:
							receipt === undefined
								? Object.freeze([])
								: diagnosticReceiptExpectations(receipt)
					})
				);
			}
			const scope = this.#protocolGeneration;
			diagnostics.update(
				Object.freeze({
					scope:
						scope === undefined
							? Object.freeze({
									generation: this.#protocolGenerationSequence,
									established: false
								})
							: Object.freeze({
									generation: this.#protocolGenerationSequence,
									established: true,
									protocolVersion: 2 as const,
									schemaHash: scope.schemaHash
								}),
					records: Object.freeze(records),
					indexes: Object.freeze(indexes),
					layers: Object.freeze([
						...(this.#diagnosticLayers?.values() ?? [])
					]),
					receipts: Object.freeze(receipts)
				})
			);
		} catch (error) {
			reportSafely(
				this.#reportObserverError,
				new AggregateError([error], 'replica diagnostics update failed')
			);
		}
	}

	#diagnosticEvent(event: ReplicaDiagnosticEventInput): void {
		if (this.#diagnostics === undefined) return;
		try {
			this.#diagnostics.event(event);
		} catch (error) {
			reportSafely(
				this.#reportObserverError,
				new AggregateError([error], 'replica diagnostics event failed')
			);
		}
	}

	#diagnosticOperation<TData, TVariables extends GraphqlVariables>(
		artifact: ReplicaOperationArtifact<TData, TVariables>
	): void {
		if (this.#diagnostics === undefined) return;
		try {
			this.#diagnostics.operation(artifact);
		} catch (error) {
			reportSafely(
				this.#reportObserverError,
				new AggregateError([error], 'replica diagnostic artifact failed')
			);
		}
	}

	#diagnosticScopeTransition(
		previous: ProtocolGeneration | undefined,
		next: ProtocolGeneration
	): void {
		if (this.#diagnostics === undefined) return;
		if (
			previous !== undefined &&
			previous.cacheScope === next.cacheScope &&
			previous.schemaHash === next.schemaHash
		) {
			return;
		}
		this.#diagnosticEvent(
			Object.freeze({
				kind: 'scope',
				action: previous === undefined ? 'established' : 'changed',
				generation: this.#protocolGenerationSequence,
				schemaHash: next.schemaHash
			})
		);
	}

	#retireDiagnosticLayer(
		id: string,
		action: 'retired' | 'rejected',
		receiptState: 'projected' | 'rejected',
		receipt?: OptimisticReceiptState
	): void {
		const layers = this.#diagnosticLayers;
		const removed = layers?.get(id);
		if (removed === undefined) return;
		layers!.delete(id);
		this.#diagnosticEvent(
			Object.freeze({
				kind: 'layer',
				layer: id,
				action,
				recordChanges: removed.recordChanges,
				indexChanges: removed.indexChanges
			})
		);
		const causal = receipt ?? this.#optimisticReceipts.get(id);
		const counts = diagnosticReceiptCounts(causal);
		this.#diagnosticEvent(
			Object.freeze({
				kind: 'receipt',
				command: id,
				state: receiptState,
				obligations: counts.obligations,
				observed: counts.observed
			})
		);
		for (const layer of layers!.values()) {
			if (layer.sequence <= removed.sequence) continue;
			this.#diagnosticEvent(
				Object.freeze({
					kind: 'layer',
					layer: layer.id,
					action: 'rebased',
					recordChanges: layer.recordChanges,
					indexChanges: layer.indexChanges,
					reason: `${action}-earlier-layer`
				})
			);
		}
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

	#stageProtocolGeneration(
		envelope: DistributedProtocolEnvelope
	): ProtocolGeneration {
		const next: ProtocolGeneration = {
			protocolVersion: 2,
			cacheScope: envelope.cacheScope,
			schemaHash: envelope.schemaHash
		};
		if (this.#protocolGeneration === undefined) return next;
		if (
			this.#protocolGeneration.cacheScope === next.cacheScope &&
			this.#protocolGeneration.schemaHash === next.schemaHash
		) {
			return this.#protocolGeneration;
		}
		this.#purgeProtocolGeneration();
		return next;
	}

	#stageTrustedPresets<TData, TVariables extends GraphqlVariables>(
		incoming: readonly DistributedTrustedPreset[],
		nextGeneration: ProtocolGeneration,
		artifact: ReplicaOperationArtifact<TData, TVariables>
	): readonly DistributedTrustedPreset[] {
		const presets = canonicalTrustedPresets(incoming);
		try {
			const binding = validatedArtifactBinding(artifact);
			const operationDescriptors = binding.trustedPresets;
			const commandDescriptors =
				this.#commandAuthorityContract?.trustedPresets;
			if (
				commandDescriptors !== undefined &&
				trustedPresetDescriptorFingerprint(operationDescriptors) !==
					trustedPresetDescriptorFingerprint(commandDescriptors)
			) {
				throw new TypeError(
					'operation and command trusted preset contracts differ'
				);
			}
			matchReplicaTrustedPresetInventory(operationDescriptors, presets);
			if (
				this.#protocolGeneration === nextGeneration &&
				trustedPresetInventoryFingerprint(this.#trustedPresets) !==
					trustedPresetInventoryFingerprint(presets)
			) {
				throw new TypeError(
					'trusted presets changed within one authoritative cache scope'
				);
			}
			return presets;
		} catch {
			if (this.#protocolGeneration !== undefined) {
				this.#purgeProtocolGeneration();
			}
			protocolInvalid('extensions.distributed.trustedPresets');
		}
	}

	#purgeProtocolGeneration(): void {
		this.#closeAuthorizationGeneration();
		this.#queryStates.clear();
		this.#operationProtocols.clear();
		this.#operationGenerations.clear();
		this.#recordClocks.clear();
		this.#recordKeysByScope.clear();
		this.#anonymousRecordClocks.clear();
		this.#optimisticReceipts.clear();
		this.#diagnosticLayers?.clear();
		this.#diagnosticLayerSequence = 0;
		this.#indexMaintenance.clear();
		this.#indexPlanRegistrations.clear();
		this.#renderedOperations.clear();
		this.#readOperationKeys.clear();
		this.#watchRenderCounts.clear();
		this.#trustedPresets = EMPTY_TRUSTED_PRESETS;
		this.#nextIndexRevision = '0';
		this.#protocolGeneration = undefined;
		this.#engine.restore(EMPTY_CACHE_SNAPSHOT);
		this.#syncDiagnostics();
		for (const watches of this.#watches.values()) {
			for (const watch of watches) {
				this.#rememberRenderedOperation(
					watch.key,
					watch.artifact,
					watch.variables,
					'watch'
				);
			}
		}
		for (const key of this.#watches.keys()) this.#emitState(key, true);
		if (this.#diagnostics !== undefined) {
			this.#diagnosticEvent(
				Object.freeze({
					kind: 'scope',
					action: 'invalidated',
					generation: this.#protocolGenerationSequence
				})
			);
		}
	}

	#closeAuthorizationGeneration(): void {
		this.#protocolGenerationSequence += 1;
		this.#authorizationAbort.abort();
		this.#authorizationAbort = new AbortController();
		this.#closeActiveTransports();
	}

	#closeActiveTransports(): void {
		for (const controller of this.#inFlightAborts.values()) controller.abort();
		this.#inFlightAborts.clear();
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
	}

	#rememberRenderedOperation<TData, TVariables extends GraphqlVariables>(
		key: string,
		artifact: ReplicaOperationArtifact<TData, TVariables>,
		variables: TVariables,
		source: 'read' | 'watch'
	): void {
		this.#diagnosticOperation(artifact);
		if (!this.#indexPlanRegistrations.has(key)) {
			const registration = this.#indexMaintenance.registerOperation(
				artifact,
				variables
			);
			this.#indexPlanRegistrations.set(key, registration);
			try {
				this.#refreshIndexMaintenance();
			} catch (error) {
				this.#indexPlanRegistrations.delete(key);
				registration.dispose();
				throw error;
			}
		}
		this.#renderedOperations.set(
			key,
			Object.freeze({
				artifact: artifact as ReplicaOperationArtifact<unknown, GraphqlVariables>,
				variables
			})
		);
		if (source === 'read') {
			this.#readOperationKeys.add(key);
		} else {
			this.#watchRenderCounts.set(
				key,
				(this.#watchRenderCounts.get(key) ?? 0) + 1
			);
		}
	}

	#forgetRenderedWatch(key: string): void {
		const count = this.#watchRenderCounts.get(key);
		if (count === undefined) return;
		if (count > 1) {
			this.#watchRenderCounts.set(key, count - 1);
			return;
		}
		this.#watchRenderCounts.delete(key);
		if (!this.#readOperationKeys.has(key)) {
			this.#renderedOperations.delete(key);
			this.#disposeIndexPlan(key);
		}
	}

	#finishDehydration(): void {
		let plansChanged = false;
		for (const key of this.#readOperationKeys) {
			if (!this.#watchRenderCounts.has(key)) {
				this.#renderedOperations.delete(key);
				const registration = this.#indexPlanRegistrations.get(key);
				if (registration !== undefined) {
					this.#indexPlanRegistrations.delete(key);
					registration.dispose();
					plansChanged = true;
				}
			}
		}
		this.#readOperationKeys.clear();
		if (plansChanged) this.#refreshIndexMaintenance();
	}

	#disposeIndexPlan(key: string): void {
		const registration = this.#indexPlanRegistrations.get(key);
		if (registration === undefined) return;
		this.#indexPlanRegistrations.delete(key);
		registration.dispose();
		this.#refreshIndexMaintenance();
	}

	#refreshIndexMaintenance(): void {
		this.#engine.setDerivedIndexReconciler(this.#derivedIndexReconciler);
	}

	#deriveMaintainedIndexes(
		confirmed: CacheEngineSnapshot,
		layers: readonly OptimisticLayerView[]
	): readonly DerivedIndexMutation[] {
		const snapshot = indexMaintenanceSnapshot(confirmed);
		const indexes = new Map(snapshot.indexes.map((index) => [index.key, index]));
		const semanticLayers = layers.map(indexSemanticLayer);
		const mutations: DerivedIndexMutation[] = [];
			for (const decision of this.#indexMaintenance.evaluate(
				snapshot,
				semanticLayers,
				this.#trustedPresets
			)) {
			if (decision.kind === 'unchanged') continue;
			if (decision.kind === 'stale') {
				mutations.push({
					kind: 'stale',
					key: decision.indexKey,
					reason: formatReplicaIndexStaleReason(decision.reason)
				});
				continue;
			}
			const index = indexes.get(decision.indexKey);
			if (index === undefined) {
				throw new Error(
					`index maintenance returned an unknown index: ${decision.indexKey}`
				);
			}
			mutations.push({
				kind: 'write',
				write: {
					key: decision.indexKey,
					records: decision.records,
					complete: true,
					metadata: index.metadata
				}
			});
		}
		return Object.freeze(mutations);
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
			const discardedStates = new Set(
				[group.query, group.live].filter(
					(state): state is OperationProtocolState =>
						state !== undefined
				)
			);
			const revisionsByKey = new Map<string, Set<string>>();
			for (const state of [group.query, group.live]) {
				if (state?.indexRevision === undefined) continue;
				for (const indexKey of state.indexKeys) {
					let revisions = revisionsByKey.get(indexKey);
					if (revisions === undefined) {
						revisions = new Set();
						revisionsByKey.set(indexKey, revisions);
					}
					revisions.add(state.indexRevision);
				}
			}
			for (const root of artifact.roots) {
				const indexKey = replicaIndexKey({
					field: root.field,
					arguments: resolveArguments(
						root.arguments,
						variables,
						root.coverage
					)
				});
				let revisions = revisionsByKey.get(indexKey);
				if (revisions === undefined) {
					revisions = new Set();
					revisionsByKey.set(indexKey, revisions);
				}
				for (const state of [group.query, group.live]) {
					if (state?.indexRevision !== undefined) {
						revisions.add(state.indexRevision);
					}
				}
			}
			const confirmedFences = this.#confirmedIndexFences(
				revisionsByKey.keys()
			);
			const ownedKeys = [...revisionsByKey].flatMap(
				([indexKey, revisions]) => {
					const revision = confirmedFences.get(indexKey);
					return revision !== undefined &&
						revisions.has(revision) &&
						!this.#indexClaimedByAnotherOperationState(
							indexKey,
							revision,
							discardedStates
						)
						? [indexKey]
						: [];
				}
			);
			this.#engine.discardIndexes(ownedKeys);
		}
		group.active = source;
		this.#advanceOperationGeneration(key);
		return previous !== undefined;
	}

	#operationGeneration(key: string): number {
		return this.#operationGenerations.get(key) ?? 0;
	}

	#indexClaimedByAnotherOperationState(
		indexKey: string,
		revision: string,
		excluded: ReadonlySet<OperationProtocolState>
	): boolean {
		for (const group of this.#operationProtocols.values()) {
			for (const state of [group.query, group.live]) {
				if (
					state !== undefined &&
					!excluded.has(state) &&
					state.indexRevision === revision &&
					state.indexKeys.has(indexKey)
				) {
					return true;
				}
			}
		}
		return false;
	}

	#confirmedIndexFences(
		indexKeys: Iterable<string>
	): ReadonlyMap<string, string> {
		return this.#engine.confirmedIndexFences([...indexKeys]);
	}

	#sharedIndexDisposition(
		currentKey: string,
		incomingIndexKeys: ReadonlySet<string>,
		snapshot: DistributedQuerySnapshot,
		requestRevision: string | undefined,
		source: ReplicaWriteSource
	): SharedIndexDisposition {
		if (incomingIndexKeys.size === 0) return { compared: false };
		const confirmedRevisions =
			this.#confirmedIndexFences(incomingIndexKeys);
		let compared = false;
		let lower = false;
		let higher = false;
		let incomparable = false;
		let equalRevision: string | undefined;
		let latestOwnerRevision: string | undefined;
		for (const [key, group] of this.#operationProtocols) {
			if (key === currentKey) continue;
			for (const state of [group.query, group.live]) {
				if (state?.indexRevision === undefined) continue;
				let ownsIncomingIndex = false;
				for (const indexKey of state.indexKeys) {
					if (
						incomingIndexKeys.has(indexKey) &&
						confirmedRevisions.get(indexKey) === state.indexRevision
					) {
						ownsIncomingIndex = true;
						break;
					}
				}
				if (!ownsIncomingIndex) continue;
				latestOwnerRevision =
					latestOwnerRevision === undefined ||
					compareCanonicalDecimalStrings(
						state.indexRevision,
						latestOwnerRevision
					) > 0
						? state.indexRevision
						: latestOwnerRevision;
				/*
				 * Snapshot scope is bound to one operation plan instance and is
				 * not a cross-artifact identity. Shared semantic indexes are
				 * comparable through their projection/scope/position vector.
				 */
				const disposition =
					!snapshot.indexesComparable
						? 'incomparable'
						: state.indexClocks.size === 0 ||
							  snapshot.indexes.length === 0
							? state.indexClocks.size === snapshot.indexes.length
								? 'equal'
								: 'incomparable'
							: compareIndexVector(
									state.indexClocks,
									snapshot.indexes
								);
				if (
					disposition === 'fresh' ||
					disposition === 'incomparable'
				) {
					incomparable = true;
					continue;
				}
				compared = true;
				if (disposition === 'lower') lower = true;
				else if (disposition === 'higher') higher = true;
				else {
					equalRevision =
						equalRevision === undefined ||
						compareCanonicalDecimalStrings(
							state.indexRevision,
							equalRevision
						) > 0
							? state.indexRevision
							: equalRevision;
				}
			}
		}
		/*
		 * One response is a coherent replacement graph. If its vector straddles
		 * the owners of two shared indexes, accepting only part would fabricate
		 * a snapshot the server never produced, so preserve the current graph.
		 */
		if (lower) return { compared: true, disposition: 'lower' };
		if (incomparable) {
			/*
			 * A comparable sibling must not promote an incomparable membership
			 * to response-arrival order. Fall back to request-start order for
			 * the whole graph, and reject it atomically when that request began
			 * before any owner it would replace. Independent live streams have
			 * no shared request-start fence, so they cannot safely replace it;
			 * explicit synchronous ingress retains its caller-defined order.
			 */
			if (requestRevision === undefined && source === 'live') {
				return { compared: true, disposition: 'lower' };
			}
			if (requestRevision === undefined) return { compared: false };
			return latestOwnerRevision !== undefined &&
				compareCanonicalDecimalStrings(requestRevision, latestOwnerRevision) <
					0
				? { compared: true, disposition: 'lower' }
				: { compared: false };
		}
		if (!compared) return { compared: false };
		if (higher) return { compared: true, disposition: 'higher' };
		return {
			compared: true,
			disposition: 'equal',
			...(equalRevision === undefined
				? {}
				: { indexRevision: equalRevision })
		};
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
					arguments: resolveArguments(
						root.arguments,
						variables,
						root.coverage
					)
				})
			);
		}
		const indexRevision = state.indexRevision;
		const discardedStates = new Set([state]);
		const confirmedFence = this.#confirmedIndexFences(keys);
		const ownedKeys =
			indexRevision === undefined
				? []
				: [...keys].filter(
						(key) =>
							confirmedFence.get(key) === indexRevision &&
							!this.#indexClaimedByAnotherOperationState(
								key,
								indexRevision,
								discardedStates
							)
					);
		this.#engine.discardIndexes(ownedKeys);
		this.#resetOperationState(state);
	}

	#resetOperationState(state: OperationProtocolState): void {
		this.#resetOperationCausalState(state);
		state.indexRevision = undefined;
		state.indexKeys.clear();
		state.pathRecords.clear();
	}

	#resetOperationCausalState(state: OperationProtocolState): void {
		state.snapshotScope = undefined;
		state.indexClocks = new Map();
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
		if (!snapshot.indexesComparable) {
			protocolInvalid(
				'extensions.distributed.snapshot.indexesComparable'
			);
		}
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
			const layer = this.#diagnosticLayers?.get(id);
			if (layer !== undefined) {
				this.#diagnosticLayers!.set(
					id,
					Object.freeze({ ...layer, state: 'accepted' as const })
				);
			}
			if (this.#diagnostics !== undefined) {
				const counts = diagnosticReceiptCounts(receipt);
				this.#diagnosticEvent(
					Object.freeze({
						kind: 'receipt',
						command: id,
						state:
							counts.obligations === 0
								? ('accepted' as const)
								: ('accepted_pending_projection' as const),
						obligations: counts.obligations,
						observed: counts.observed
					})
				);
			}
		}
	}

	/** Package-internal hook used by one watched operation. */
	_register<TData, TVariables extends GraphqlVariables>(
		watch: ReplicaWatchState<TData, TVariables>
	): () => void {
		this.#rememberRenderedOperation(
			watch.key,
			watch.artifact,
			watch.variables,
			'watch'
		);
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
			(materialized) => watch._cacheChanged(materialized),
			{ immediate: true }
		);
		if (watch.liveRequested) this.#retainLive(watch);
		void this._fetch(watch, false);
		return () => {
			unwatch();
			watches?.delete(watch as ReplicaWatchState<unknown, GraphqlVariables>);
			if (watches?.size === 0) this.#watches.delete(watch.key);
			this.#forgetRenderedWatch(watch.key);
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
			if (this.#diagnostics !== undefined) {
				this.#diagnosticEvent(
					Object.freeze({
						kind: 'revalidation',
						operation: watch.artifact.id,
						action: 'skipped-complete',
						reason: 'watch'
					})
				);
			}
			return Promise.resolve();
		}
		const existing = this.#inFlight.get(watch.key);
		if (existing) {
			if (this.#diagnostics !== undefined) {
				this.#diagnosticEvent(
					Object.freeze({
						kind: 'revalidation',
						operation: watch.artifact.id,
						action: 'deduplicated',
						reason: force
							? ('refresh' as const)
							: watch.materialized.stale
								? ('stale' as const)
								: ('watch' as const)
					})
				);
			}
			return existing;
		}

		const state = this.#queryState(watch.key);
		if (this.#diagnostics !== undefined) {
			this.#diagnosticEvent(
				Object.freeze({
					kind: 'revalidation',
					operation: watch.artifact.id,
					action: 'requested',
					reason: force
						? ('refresh' as const)
						: watch.materialized.stale
							? ('stale' as const)
							: ('watch' as const)
				})
			);
		}
		state.fetching = true;
		this.#emitState(watch.key, false);
		const operationGeneration = this.#operationGeneration(watch.key);
		const authorizationGeneration = this.#protocolGenerationSequence;
		/*
		 * Reserve local ordering when the request starts, not when it finishes.
		 * Distinct operation artifacts may share the same semantic index key;
		 * a slower earlier request must not replace a later-started result merely
		 * because its response arrived last.
		 */
		const requestRevision = this.#allocateIndexRevision();
		const controller = new AbortController();
		const request = Object.freeze({
			operation: 'query' as const,
			operationId: watch.artifact.id,
			document: watch.artifact.document,
			variables: watch.variables,
			artifact: watch.artifact,
			...replicaClientRequestExtensions(watch.artifact),
			signal: controller.signal
		});
		let flight: Promise<void>;
		flight = Promise.resolve()
			.then(() => this.#transport!.fetch(request))
			.then((result) => {
				if (this.#protocolGenerationSequence !== authorizationGeneration) {
					if (this.#diagnostics !== undefined) {
						this.#diagnosticEvent(
							Object.freeze({
								kind: 'response-fenced',
								operation: watch.artifact.id,
								transport: 'http',
								reason: 'authorization-generation'
							})
						);
					}
					return;
				}
				if (this.#inFlight.get(watch.key) !== flight) {
					if (this.#diagnostics !== undefined) {
						this.#diagnosticEvent(
							Object.freeze({
								kind: 'response-fenced',
								operation: watch.artifact.id,
								transport: 'http',
								reason: 'superseded'
							})
						);
					}
					return;
				}
				if (this.#operationGeneration(watch.key) !== operationGeneration) {
					if (this.#diagnostics !== undefined) {
						this.#diagnosticEvent(
							Object.freeze({
								kind: 'response-fenced',
								operation: watch.artifact.id,
								transport: 'http',
								reason: 'operation-generation'
							})
						);
					}
					return;
				}
				this.#writeCanonicalResult(
					watch.artifact,
					watch.variables,
					result,
					'network',
					requestRevision
				);
			})
			.catch((error: unknown) => {
				if (this.#inFlight.get(watch.key) !== flight) return;
				if (controller.signal.aborted) return;
				state.errors = stableErrors(state.errors, [graphqlError(error)]);
			})
			.finally(() => {
				if (this.#inFlight.get(watch.key) !== flight) return;
				this.#inFlight.delete(watch.key);
				this.#inFlightAborts.delete(watch.key);
				state.fetching = false;
				this.#emitState(watch.key, false);
			});
		this.#inFlight.set(watch.key, flight);
		this.#inFlightAborts.set(watch.key, controller);
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
					...replicaClientRequestExtensions(watch.artifact),
					...(resume === undefined || resume.length === 0
						? {}
						: { resume })
				}),
				{
					next: (result) => {
						if (
							entry.protocolGeneration !== this.#protocolGenerationSequence
						) {
							if (this.#diagnostics !== undefined) {
								this.#diagnosticEvent(
									Object.freeze({
										kind: 'response-fenced',
										operation: watch.artifact.live!.id,
										transport: 'live',
										reason: 'authorization-generation'
									})
								);
							}
							return;
						}
						if (
							entry.operationGeneration !== undefined &&
							entry.operationGeneration !==
								this.#operationGeneration(watch.key)
						) {
							if (this.#diagnostics !== undefined) {
								this.#diagnosticEvent(
									Object.freeze({
										kind: 'response-fenced',
										operation: watch.artifact.live!.id,
										transport: 'live',
										reason: 'operation-generation'
									})
								);
							}
							return;
						}
						if (
							!entry.active ||
							this.#lives.get(watch.key) !== entry
						) {
							if (this.#diagnostics !== undefined) {
								this.#diagnosticEvent(
									Object.freeze({
										kind: 'response-fenced',
										operation: watch.artifact.live!.id,
										transport: 'live',
										reason: 'superseded'
									})
								);
							}
							return;
						}
						let unsupportedLive = false;
						try {
							unsupportedLive =
								parseGraphqlResponseExtensions(result.extensions)
									?.distributed?.live?.supported === false;
							if (unsupportedLive) state.live = 'off';
							const distributed = this.#writeCanonicalResult(
								watch.artifact,
								watch.variables,
								result,
								'live'
							);
							if (distributed.live?.supported === false) {
								this.#fallbackFromLive(watch, entry);
								return;
							}
							state.live = 'active';
							entry.operationGeneration =
								this.#operationGeneration(watch.key);
						} catch (error) {
							if (
								unsupportedLive &&
								error instanceof CacheRevisionConflictError
							) {
								/*
								 * Revision zero is shared by provisional fallbacks.
								 * Another operation may already have filled the same
								 * semantic index differently; HTTP remains authoritative.
								 */
								this.#fallbackFromLive(watch, entry);
								return;
							}
							state.live = 'error';
							state.errors = stableErrors(state.errors, [graphqlError(error)]);
							this.#emitState(watch.key, false);
						}
					},
					error: (error) => {
						if (!entry.active || this.#lives.get(watch.key) !== entry) return;
						entry.active = false;
						this.#lives.delete(watch.key);
						const unsubscribe = entry.unsubscribe;
						entry.unsubscribe = () => undefined;
						try {
							unsubscribe();
						} catch {
							// The terminal stream is fenced; cleanup is best effort.
						}
						state.live = 'error';
						state.errors = stableErrors(state.errors, [graphqlError(error)]);
						this.#emitState(watch.key, false);
					},
					complete: () => {
						if (!entry.active || this.#lives.get(watch.key) !== entry) return;
						this.#fallbackFromLive(watch, entry);
					}
				}
			);
			entry.unsubscribe = unsubscribe;
			if (!entry.active || this.#lives.get(watch.key) !== entry) {
				entry.unsubscribe = () => undefined;
				try {
					unsubscribe();
				} catch {
					// A closed/superseded subscription is already fenced.
				}
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

	#fallbackFromLive<TData, TVariables extends GraphqlVariables>(
		watch: ReplicaWatchState<TData, TVariables>,
		entry: LiveEntry
	): void {
		if (this.#lives.get(watch.key) !== entry) {
			void this._fetch(watch, true);
			return;
		}
		const protocol = this.#operationProtocols.get(watch.key);
		if (protocol?.active === 'live') protocol.active = undefined;
		entry.active = false;
		const unsubscribe = entry.unsubscribe;
		entry.unsubscribe = () => undefined;
		try {
			unsubscribe();
		} catch {
			// The inactive stream is fenced; transport cleanup is best effort.
		}
		const state = this.#queryState(watch.key);
		state.live = 'off';
		this.#emitState(watch.key, false);
		const authorizationGeneration = this.#protocolGenerationSequence;
		const supersededFlight =
			entry.operationGeneration === undefined
				? undefined
				: this.#inFlight.get(watch.key);
		const refresh = (): void => {
			if (
				this.#protocolGenerationSequence !== authorizationGeneration ||
				this.#lives.get(watch.key) !== entry ||
				entry.active
			) {
				return;
			}
			void this._fetch(watch, true);
		};
		/*
		 * Keep the inactive entry as an authorization-generation-scoped
		 * sentinel. Query ingestion calls #resumeLiveWatches(); deleting this
		 * entry would otherwise reopen an unsupported or completed stream
		 * immediately. Authorization invalidation clears it and may retry.
		 *
		 * A supported live frame advances the operation generation. Any HTTP
		 * request that was already running is therefore doomed by its response
		 * fence; drain it before starting the authoritative fallback. A first
		 * unsupported frame never advances the generation, so its overlapping
		 * HTTP request remains valid and can be reused directly.
		 */
		if (supersededFlight === undefined) {
			refresh();
		} else {
			void supersededFlight.then(refresh, refresh);
		}
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

	#resumeLiveWatches(): void {
		if (this.#protocolGeneration === undefined) return;
		for (const [key, watches] of this.#watches) {
			if (this.#lives.has(key)) continue;
			const liveWatches = [...watches].filter(
				(watch) => watch.liveRequested
			);
			const first = liveWatches[0];
			if (first === undefined) continue;
			this.#retainLive(first);
			const entry = this.#lives.get(key);
			if (entry !== undefined) entry.count = liveWatches.length;
		}
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

function replicaResultIndexKeys<
	TData,
	TVariables extends GraphqlVariables
>(
	artifact: ReplicaOperationArtifact<TData, TVariables>,
	variables: TVariables,
	envelope: ReplicaResultEnvelope<TData>,
	snapshot: DistributedQuerySnapshot
): ReadonlySet<string> {
	const keys = new Set<string>();
	if (
		envelope.data === undefined ||
		envelope.data === null ||
		!isReplicaResultObject(envelope.data) ||
		(envelope.errors ?? []).some(
			(error) => !Array.isArray(error.path) || error.path.length === 0
		)
	) {
		return keys;
	}
	const errorPaths = (envelope.errors ?? []).flatMap((error) =>
		Array.isArray(error.path) && error.path.length > 0
			? [error.path]
			: []
	);
	const evidencePaths = new Set(
		snapshot.records.flatMap((record) =>
			record.path === undefined || record.tombstone
				? []
				: [responsePathKey(record.path)]
		)
	);
	for (const artifactRoot of artifact.roots) {
		const root = runtimeRoot(artifactRoot);
		const rootPath: readonly (string | number)[] = [root.responseKey];
		const rootKey = replicaIndexKey({
			field: root.field,
			arguments: resolveArguments(
				root.arguments,
				variables,
				root.coverage
			)
		});
		keys.add(rootKey);
		if (
			resultPathBlocked(errorPaths, rootPath) ||
			!Object.prototype.hasOwnProperty.call(
				envelope.data,
				root.responseKey
			)
		) {
			continue;
		}
		const value = envelope.data[root.responseKey];
		if (
			value === null &&
			resultPathHasErrors(errorPaths, rootPath)
		) {
			continue;
		}
		collectResultBranchIndexKeys(
			artifact.id,
			root,
			value,
			rootPath,
			rootKey,
			variables,
			errorPaths,
			evidencePaths,
			keys
		);
	}
	return keys;
}

function collectResultBranchIndexKeys(
	artifactId: string,
	selection: RuntimeRootSelection | RuntimeObjectBranch,
	value: unknown,
	path: readonly (string | number)[],
	enclosingIndexKey: string,
	variables: GraphqlVariables,
	errorPaths: readonly (readonly (string | number)[])[],
	evidencePaths: ReadonlySet<string>,
	keys: Set<string>
): void {
	if (value === null || value === undefined) return;
	if (selection.cardinality === 'one') {
		collectResultObjectIndexKeys(
			artifactId,
			selection.selection,
			value,
			path,
			enclosingIndexKey,
			undefined,
			variables,
			errorPaths,
			evidencePaths,
			keys
		);
		return;
	}
	if (!Array.isArray(value)) return;
	for (const [ordinal, entry] of value.entries()) {
		if (entry === null || entry === undefined) continue;
		collectResultObjectIndexKeys(
			artifactId,
			selection.selection,
			entry,
			[...path, ordinal],
			enclosingIndexKey,
			ordinal,
			variables,
			errorPaths,
			evidencePaths,
			keys
		);
	}
}

function collectResultObjectIndexKeys(
	artifactId: string,
	selection: RuntimeObjectSelection,
	value: unknown,
	path: readonly (string | number)[],
	enclosingIndexKey: string,
	ordinal: number | undefined,
	variables: GraphqlVariables,
	errorPaths: readonly (readonly (string | number)[])[],
	evidencePaths: ReadonlySet<string>,
	keys: Set<string>
): void {
	if (resultPathBlocked(errorPaths, path) || !isReplicaResultObject(value)) {
		return;
	}
	const fields = new Map<string, CacheValue>();
	for (const member of selection.members) {
		if (member.kind !== 'scalar') continue;
		const fieldPath = [...path, member.responseKey];
		if (
			resultPathBlocked(errorPaths, fieldPath) ||
			!Object.prototype.hasOwnProperty.call(value, member.responseKey)
		) {
			continue;
		}
		const rawValue = value[member.responseKey];
		if (rawValue === null && !member.nullable) continue;
		if (!fields.has(member.field)) {
			fields.set(
				member.field,
				cloneJsonValue(rawValue) as CacheValue
			);
		}
	}
	let parentKey: string;
	if (
		selection.storage.kind === 'normalized' &&
		evidencePaths.has(responsePathKey(path.map(String)))
	) {
		const identity = selection.storage.identityFields.flatMap((field) => {
			const value = fields.get(field);
			return value === undefined || value === null ? [] : [value];
		});
		if (identity.length !== selection.storage.identityFields.length) return;
		parentKey = replicaRecordKey(
			{
				id: selection.storage.model,
				identityFields: selection.storage.identityFields
			},
			identity
		);
	} else {
		parentKey = embeddedRecordKey(
			artifactId,
			enclosingIndexKey,
			ordinal
		);
	}
	for (const member of selection.members) {
		if (member.kind !== 'branch') continue;
		const branchPath = [...path, member.responseKey];
		const branchKey = replicaIndexKey({
			parent: parentKey,
			field: member.field,
			arguments: resolveArguments(
				member.arguments,
				variables,
				member.coverage
			)
		});
		keys.add(branchKey);
		if (
			resultPathBlocked(errorPaths, branchPath) ||
			!Object.prototype.hasOwnProperty.call(value, member.responseKey)
		) {
			continue;
		}
		const branchValue = value[member.responseKey];
		if (
			branchValue === null &&
			resultPathHasErrors(errorPaths, branchPath)
		) {
			continue;
		}
		collectResultBranchIndexKeys(
			artifactId,
			member,
			branchValue,
			branchPath,
			branchKey,
			variables,
			errorPaths,
			evidencePaths,
			keys
		);
	}
}

function resultPathBlocked(
	errorPaths: readonly (readonly (string | number)[])[],
	path: readonly (string | number)[]
): boolean {
	return errorPaths.some((errorPath) => resultPathPrefix(errorPath, path));
}

function resultPathHasErrors(
	errorPaths: readonly (readonly (string | number)[])[],
	path: readonly (string | number)[]
): boolean {
	return errorPaths.some(
		(errorPath) =>
			resultPathPrefix(path, errorPath) ||
			resultPathPrefix(errorPath, path)
	);
}

function resultPathPrefix(
	prefix: readonly (string | number)[],
	value: readonly (string | number)[]
): boolean {
	return (
		prefix.length <= value.length &&
		prefix.every((entry, index) => entry === value[index])
	);
}

function isReplicaResultObject(
	value: unknown
): value is Readonly<Record<string, unknown>> {
	return value !== null && typeof value === 'object' && !Array.isArray(value);
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

function modelFromRecordKey(recordKey: string): string | undefined {
	if (!recordKey.startsWith('record:')) return undefined;
	const separator = recordKey.indexOf(':', 'record:'.length);
	if (separator === -1) return undefined;
	try {
		const model = decodeURIComponent(
			recordKey.slice('record:'.length, separator)
		);
		return model.length === 0 ? undefined : model;
	} catch {
		return undefined;
	}
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

function diagnosticReceiptExpectations(
	receipt: OptimisticReceiptState
): readonly {
	readonly projection: string;
	readonly model: string;
	readonly observed: boolean;
}[] {
	return Object.freeze(
		[...receipt.expectations.keys()]
			.map((key) => {
				const parsed = JSON.parse(key) as unknown;
				if (
					!Array.isArray(parsed) ||
					parsed.length !== 3 ||
					typeof parsed[0] !== 'string' ||
					typeof parsed[1] !== 'string'
				) {
					throw new TypeError('invalid internal projection expectation');
				}
				return Object.freeze({
					projection: parsed[0],
					model: parsed[1],
					observed: receipt.observed.has(key)
				});
			})
			.sort((left, right) =>
				`${left.projection}\0${left.model}`.localeCompare(
					`${right.projection}\0${right.model}`
				)
			)
	);
}

function diagnosticReceiptCounts(
	receipt: OptimisticReceiptState | undefined
): Readonly<{ obligations: number; observed: number }> {
	return Object.freeze({
		obligations: receipt?.expectations.size ?? 0,
		observed: receipt?.observed.size ?? 0
	});
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

function compareCanonicalDecimalStrings(
	left: string,
	right: string
): number {
	return left.length === right.length
		? left.localeCompare(right)
		: left.length < right.length
			? -1
			: 1;
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
		this.variables = canonicalizeOperationVariables(artifact, variables);
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

function serializeOperationProtocolGroup(
	key: string,
	group: OperationProtocolGroup,
	generation: number
): SerializedOperationProtocolGroup {
	return Object.freeze({
		key,
		...(group.query === undefined
			? {}
			: { query: serializeOperationProtocolState(group.query) }),
		...(group.live === undefined
			? {}
			: { live: serializeOperationProtocolState(group.live) }),
		...(group.active === undefined ? {} : { active: group.active }),
		generation
	});
}

function serializeOperationProtocolState(
	state: OperationProtocolState
): SerializedOperationProtocolState {
	return Object.freeze({
		operation: state.operation,
		...(state.snapshotScope === undefined
			? {}
			: { snapshotScope: state.snapshotScope }),
		indexClocks: Object.freeze(
			[...state.indexClocks]
				.sort(([left], [right]) => left.localeCompare(right))
				.map(([projection, clock]) =>
					Object.freeze([
						projection,
						Object.freeze({
							scopeToken: clock.scopeToken,
							position: clock.position
						})
					] as const)
				)
		),
		...(state.indexRevision === undefined
			? {}
			: { indexRevision: state.indexRevision }),
		indexKeys: Object.freeze([...state.indexKeys].sort()),
		pathRecords: Object.freeze(
			[...state.pathRecords]
				.sort(([left], [right]) => left.localeCompare(right))
				.map(([path, key]) => Object.freeze([path, key] as const))
		),
		cursors: Object.freeze(
			state.cursors.map((cursor) =>
				Object.freeze({
					projection: cursor.projection,
					position: cursor.position,
					token: cursor.token
				})
			)
		)
	});
}

function freezeRecordClock(clock: RecordProtocolClock): RecordProtocolClock {
	return Object.freeze({
		scopeToken: clock.scopeToken,
		incarnation: clock.incarnation,
		revision: clock.revision,
		tombstone: clock.tombstone
	});
}

function parseAuthoritativeScope(
	value: unknown
): ProtocolGeneration | undefined {
	try {
		const scope = hydrationRecord(
			value,
			'authoritativeScope',
			['protocolVersion', 'schemaHash', 'cacheScope']
		);
		if (scope.protocolVersion !== 2) {
			hydrationInvalid('authoritativeScope.protocolVersion');
		}
		return Object.freeze({
			protocolVersion: 2,
			schemaHash: hydrationString(
				scope.schemaHash,
				'authoritativeScope.schemaHash'
			),
			cacheScope: hydrationOpaque(
				scope.cacheScope,
				'authoritativeScope.cacheScope'
			)
		});
	} catch {
		return undefined;
	}
}

function hydrationMetadataConsistent(
	parsed: ParsedReplicaHydration
): boolean {
	try {
		const recordByKey = new Map(
			parsed.cache.records.map((record) => [record.key, record])
		);
		for (const record of parsed.cache.records) {
			if (modelFromRecordKey(record.key) === undefined) continue;
			const clock = parsed.recordClocks.get(record.key);
			if (
				clock === undefined ||
				record.incarnation === undefined ||
				clock.incarnation !== record.incarnation ||
				clock.revision !== record.revision
			) {
				return false;
			}
			if (clock.tombstone) {
				if (record.tombstoneRevision !== clock.revision) return false;
			} else if (record.tombstoneRevision !== undefined) {
				return false;
			}
		}
		for (const [recordKey, clock] of parsed.recordClocks) {
			const record = recordByKey.get(recordKey);
			if (
				record?.tombstoneRevision !== undefined &&
				(
					!clock.tombstone ||
					record.tombstoneRevision !== clock.revision
				)
			) {
				return false;
			}
		}

		const revisionsByIndex = new Map<string, Set<string>>();
		for (const group of parsed.operationProtocols.values()) {
			for (const state of [group.query, group.live]) {
				if (state === undefined) continue;
				for (const recordKey of state.pathRecords.values()) {
					if (
						modelFromRecordKey(recordKey) !== undefined &&
						!parsed.recordClocks.has(recordKey)
					) {
						return false;
					}
				}
				if (state.indexRevision === undefined) {
					if (state.indexKeys.size > 0) return false;
					continue;
				}
				for (const indexKey of state.indexKeys) {
					let revisions = revisionsByIndex.get(indexKey);
					if (revisions === undefined) {
						revisions = new Set();
						revisionsByIndex.set(indexKey, revisions);
					}
					revisions.add(state.indexRevision);
				}
			}
		}
		const nextIndexRevision =
			parsed.nextIndexRevision as DistributedDecimalString;
		for (const index of parsed.cache.indexes) {
			const revision = hydrationDecimal(
				index.revision,
				'state.payload.cache.indexes.revision'
			);
			if (
				compareDistributedDecimal(revision, nextIndexRevision) > 0 ||
				!revisionsByIndex.get(index.key)?.has(index.revision)
			) {
				return false;
			}
		}
		return true;
	} catch {
		return false;
	}
}

function parseReplicaHydration(
	value: unknown
): ParsedReplicaHydration | undefined {
	try {
		const state = hydrationRecord(
			value,
			'state',
			['version', 'scope', 'payload']
		);
		if (state.version !== 1) hydrationInvalid('state.version');
		const scopeValue = hydrationRecord(
			state.scope,
			'state.scope',
			['protocolVersion', 'schemaHash', 'cacheScope']
		);
		if (scopeValue.protocolVersion !== 2) {
			hydrationInvalid('state.scope.protocolVersion');
		}
		const scope: ProtocolGeneration = Object.freeze({
			protocolVersion: 2,
			schemaHash: hydrationString(
				scopeValue.schemaHash,
				'state.scope.schemaHash'
			),
			cacheScope: hydrationOpaque(
				scopeValue.cacheScope,
				'state.scope.cacheScope'
			)
		});
		const payload = hydrationRecord(
			state.payload,
			'state.payload',
			[
				'cache',
					'operations',
					'recordClocks',
					'anonymousRecordClocks',
					'trustedPresets',
					'nextIndexRevision'
				]
			);
		const cache = payload.cache as CacheEngineSnapshot;
		if (!isHydrationRecord(cache)) hydrationInvalid('state.payload.cache');
		const operationsValue = hydrationArray(
			payload.operations,
			'state.payload.operations'
		);
		const operationProtocols = new Map<string, OperationProtocolGroup>();
		const operationGenerations = new Map<string, number>();
		for (const [index, entry] of operationsValue.entries()) {
			const path = `state.payload.operations[${index}]`;
			const raw = hydrationRecord(
				entry,
				path,
				['key', 'query', 'live', 'active', 'generation'],
				['key', 'generation']
			);
			const key = hydrationString(raw.key, `${path}.key`);
			if (!key.startsWith('protocol:') || operationProtocols.has(key)) {
				hydrationInvalid(`${path}.key`);
			}
			const query =
				raw.query === undefined
					? undefined
					: parseOperationProtocolState(raw.query, `${path}.query`);
			const live =
				raw.live === undefined
					? undefined
					: parseOperationProtocolState(raw.live, `${path}.live`);
			if (query === undefined && live === undefined) hydrationInvalid(path);
			const active =
				raw.active === undefined
					? undefined
					: hydrationOperationSource(raw.active, `${path}.active`);
			if (
				(active === 'query' && query === undefined) ||
				(active === 'live' && live === undefined)
			) {
				hydrationInvalid(`${path}.active`);
			}
			operationProtocols.set(key, {
				...(query === undefined ? {} : { query }),
				...(live === undefined ? {} : { live }),
				...(active === undefined ? {} : { active })
			});
			operationGenerations.set(
				key,
				hydrationGeneration(raw.generation, `${path}.generation`)
			);
		}

		const recordClocks = new Map<string, RecordProtocolClock>();
		const recordKeysByScope = new Map<DistributedOpaqueString, string>();
		for (const [index, entry] of hydrationArray(
			payload.recordClocks,
			'state.payload.recordClocks'
		).entries()) {
			const path = `state.payload.recordClocks[${index}]`;
			const pair = hydrationPair(entry, path);
			const key = hydrationString(pair[0], `${path}[0]`);
			if (recordClocks.has(key)) hydrationInvalid(`${path}[0]`);
			const clock = parseRecordClock(pair[1], `${path}[1]`);
			if (recordKeysByScope.has(clock.scopeToken)) {
				hydrationInvalid(`${path}[1].scopeToken`);
			}
			recordClocks.set(key, clock);
			recordKeysByScope.set(clock.scopeToken, key);
		}

		const anonymousRecordClocks = new Map<
			DistributedOpaqueString,
			AnonymousRecordProtocolClock
		>();
		for (const [index, entry] of hydrationArray(
			payload.anonymousRecordClocks,
			'state.payload.anonymousRecordClocks'
		).entries()) {
			const path = `state.payload.anonymousRecordClocks[${index}]`;
			const pair = hydrationPair(entry, path);
			const scopeToken = hydrationOpaque(pair[0], `${path}[0]`);
			if (
				anonymousRecordClocks.has(scopeToken) ||
				recordKeysByScope.has(scopeToken)
			) {
				hydrationInvalid(`${path}[0]`);
			}
			const raw = hydrationRecord(
				pair[1],
				`${path}[1]`,
				['model', 'clock']
			);
			const clock = parseRecordClock(raw.clock, `${path}[1].clock`);
			if (clock.scopeToken !== scopeToken) {
				hydrationInvalid(`${path}[1].clock.scopeToken`);
			}
			anonymousRecordClocks.set(
				scopeToken,
				Object.freeze({
					model: hydrationString(raw.model, `${path}[1].model`),
					clock
					})
				);
			}
			const trustedPresets = canonicalTrustedPresets(
				parseDistributedTrustedPresetInventory(
					payload.trustedPresets,
					'state.payload.trustedPresets'
				)
			);
			const nextIndexRevision = hydrationDecimal(
				payload.nextIndexRevision,
			'state.payload.nextIndexRevision'
		);
		for (const group of operationProtocols.values()) {
			for (const operation of [group.query, group.live]) {
				if (
					operation?.indexRevision !== undefined &&
					compareDistributedDecimal(
						operation.indexRevision as DistributedDecimalString,
						nextIndexRevision
					) > 0
				) {
					hydrationInvalid('state.payload.nextIndexRevision');
				}
			}
		}
		return {
			scope,
			cache,
			operationProtocols,
			operationGenerations,
				recordClocks,
				recordKeysByScope,
				anonymousRecordClocks,
				trustedPresets,
				nextIndexRevision
			};
	} catch {
		return undefined;
	}
}

function parseOperationProtocolState(
	value: unknown,
	path: string
): OperationProtocolState {
	const raw = hydrationRecord(
		value,
		path,
		[
			'operation',
			'snapshotScope',
			'indexClocks',
			'indexRevision',
			'indexKeys',
			'pathRecords',
			'cursors'
		],
		['operation', 'indexClocks', 'indexKeys', 'pathRecords', 'cursors']
	);
	const indexClocks = new Map<string, IndexProtocolClock>();
	for (const [index, entry] of hydrationArray(
		raw.indexClocks,
		`${path}.indexClocks`
	).entries()) {
		const entryPath = `${path}.indexClocks[${index}]`;
		const pair = hydrationPair(entry, entryPath);
		const projection = hydrationString(pair[0], `${entryPath}[0]`);
		if (indexClocks.has(projection)) hydrationInvalid(`${entryPath}[0]`);
		const clock = hydrationRecord(
			pair[1],
			`${entryPath}[1]`,
			['scopeToken', 'position']
		);
		indexClocks.set(
			projection,
			Object.freeze({
					scopeToken: hydrationOpaque(
						clock.scopeToken,
						`${entryPath}[1].scopeToken`
					),
				position: hydrationDecimal(
					clock.position,
					`${entryPath}[1].position`
				)
			})
		);
	}
	const indexKeys = new Set<string>();
	for (const [index, entry] of hydrationArray(
		raw.indexKeys,
		`${path}.indexKeys`
	).entries()) {
		const key = hydrationString(entry, `${path}.indexKeys[${index}]`);
		if (indexKeys.has(key)) hydrationInvalid(`${path}.indexKeys[${index}]`);
		indexKeys.add(key);
	}
	const pathRecords = new Map<string, string>();
	for (const [index, entry] of hydrationArray(
		raw.pathRecords,
		`${path}.pathRecords`
	).entries()) {
		const entryPath = `${path}.pathRecords[${index}]`;
		const pair = hydrationPair(entry, entryPath);
		const responsePath = hydrationString(pair[0], `${entryPath}[0]`);
		if (pathRecords.has(responsePath)) hydrationInvalid(`${entryPath}[0]`);
		pathRecords.set(
			responsePath,
			hydrationString(pair[1], `${entryPath}[1]`)
		);
	}
	const cursors = hydrationArray(raw.cursors, `${path}.cursors`).map(
		(entry, index) => {
			const entryPath = `${path}.cursors[${index}]`;
			const cursor = hydrationRecord(
				entry,
				entryPath,
				['projection', 'position', 'token']
			);
			return Object.freeze({
				projection: hydrationString(
					cursor.projection,
					`${entryPath}.projection`
				),
				position: hydrationDecimal(
					cursor.position,
					`${entryPath}.position`
				),
				token: hydrationOpaque(cursor.token, `${entryPath}.token`)
			});
		}
	);
	return {
		operation: hydrationString(raw.operation, `${path}.operation`),
		...(raw.snapshotScope === undefined
			? {}
			: {
					snapshotScope: hydrationOpaque(
						raw.snapshotScope,
						`${path}.snapshotScope`
					)
				}),
		indexClocks,
		...(raw.indexRevision === undefined
			? {}
			: {
					indexRevision: hydrationDecimal(
						raw.indexRevision,
						`${path}.indexRevision`
					)
				}),
		indexKeys,
		pathRecords,
		cursors: Object.freeze(cursors)
	};
}

function parseRecordClock(value: unknown, path: string): RecordProtocolClock {
	const raw = hydrationRecord(
		value,
		path,
		['scopeToken', 'incarnation', 'revision', 'tombstone']
	);
	if (typeof raw.tombstone !== 'boolean') hydrationInvalid(`${path}.tombstone`);
	return Object.freeze({
		scopeToken: hydrationOpaque(raw.scopeToken, `${path}.scopeToken`),
		incarnation: hydrationDecimal(raw.incarnation, `${path}.incarnation`),
		revision: hydrationDecimal(raw.revision, `${path}.revision`),
		tombstone: raw.tombstone
	});
}

function hydrationRecord(
	value: unknown,
	path: string,
	allowed: readonly string[],
	required: readonly string[] = allowed
): Record<string, unknown> {
	if (!isHydrationRecord(value)) hydrationInvalid(path);
	const allowedKeys = new Set(allowed);
	for (const key of Object.keys(value)) {
		if (!allowedKeys.has(key)) hydrationInvalid(`${path}.${key}`);
	}
	for (const key of required) {
		if (!Object.prototype.hasOwnProperty.call(value, key)) {
			hydrationInvalid(`${path}.${key}`);
		}
	}
	return value;
}

function isHydrationRecord(value: unknown): value is Record<string, unknown> {
	return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function hydrationArray(value: unknown, path: string): unknown[] {
	if (!Array.isArray(value)) hydrationInvalid(path);
	return value;
}

function hydrationPair(value: unknown, path: string): readonly [unknown, unknown] {
	if (!Array.isArray(value) || value.length !== 2) hydrationInvalid(path);
	return value as unknown as readonly [unknown, unknown];
}

function hydrationString(value: unknown, path: string): string {
	if (typeof value !== 'string' || value.length === 0) hydrationInvalid(path);
	return value;
}

function hydrationOpaque(
	value: unknown,
	path: string
): DistributedOpaqueString {
	return hydrationString(value, path) as DistributedOpaqueString;
}

function hydrationDecimal(
	value: unknown,
	path: string
): DistributedDecimalString {
	const string = hydrationString(value, path);
	if (!/^(0|[1-9][0-9]*)$/.test(string)) hydrationInvalid(path);
	return string as DistributedDecimalString;
}

function hydrationGeneration(value: unknown, path: string): number {
	if (!Number.isSafeInteger(value) || (value as number) < 0) {
		hydrationInvalid(path);
	}
	return value as number;
}

function hydrationOperationSource(
	value: unknown,
	path: string
): OperationProtocolSource {
	if (value !== 'query' && value !== 'live') hydrationInvalid(path);
	return value;
}

function hydrationInvalid(path: string): never {
	throw new TypeError(`invalid replica hydration state at ${path}`);
}

export function createDistributedReplica(
	options: DistributedReplicaOptions = {}
): DistributedReplicaApi {
	return new DistributedReplicaImpl(options);
}

type CapturedReplicaOptimisticOperation =
	| {
			readonly kind: 'write-record';
			readonly write: OptimisticRecordWrite;
	  }
	| {
			readonly kind: 'tombstone-record';
			readonly key: string;
	  }
	| {
			readonly kind: 'write-index';
			readonly write: OptimisticIndexWrite;
	  }
	| {
			readonly kind: 'delete-index';
			readonly key: string;
	  };

type CapturedReplicaOptimisticUpdate = {
	readonly operations: readonly CapturedReplicaOptimisticOperation[];
	readonly context: CacheValue;
};

function captureReplicaOptimisticUpdate(
	id: string,
	update: (writer: ReplicaOptimisticWriter) => void,
	semanticChanges: readonly ReplicaIndexSemanticChange[]
): CapturedReplicaOptimisticUpdate {
	if (!Array.isArray(semanticChanges)) {
		throw new TypeError('optimistic semantic changes must be an array');
	}
	const suppliedChanges = cloneJsonValue(
		semanticChanges
	) as unknown as readonly ReplicaIndexSemanticChange[];
	const operations: CapturedReplicaOptimisticOperation[] = [];
	const changes: ReplicaIndexSemanticChange[] = [];
	let active = true;
	const assertActive = () => {
		if (!active) throw new Error('replica optimistic writer is no longer active');
	};
	const writer: ReplicaOptimisticWriter = Object.freeze({
		writeRecord(
			model: ReplicaModelArtifact,
			identity: ReplicaIdentity,
			patch: ReplicaRecordPatch
		): void {
			assertActive();
			const key = replicaRecordKey(model, identity);
			const fields = cloneOptimisticFields(patch.fields);
			const links = cloneOptimisticLinks(patch.links);
			if (Object.keys(fields).length === 0 && Object.keys(links).length === 0) {
				return;
			}
			operations.push(
				Object.freeze({
					kind: 'write-record' as const,
					write: Object.freeze({ key, fields, links })
				})
			);
			changes.push(
				Object.freeze({
					kind: 'upsert' as const,
					model: model.id,
					key,
					fields
				})
			);
		},
		tombstoneRecord(
			model: ReplicaModelArtifact,
			identity: ReplicaIdentity
		): void {
			assertActive();
			const key = replicaRecordKey(model, identity);
			operations.push(
				Object.freeze({ kind: 'tombstone-record' as const, key })
			);
			changes.push(
				Object.freeze({
					kind: 'delete' as const,
					model: model.id,
					key
				})
			);
		},
		writeIndex(target: ReplicaIndexTarget, records: readonly string[]): void {
			assertActive();
			if (!Array.isArray(records)) {
				throw new TypeError('index records must be an array');
			}
			const metadata = cloneJsonValue(
				metadataFromTarget(target)
			) as unknown as CacheIndexMetadata;
			operations.push(
				Object.freeze({
					kind: 'write-index' as const,
					write: Object.freeze({
						key: indexKeyFromTarget(target),
						records: Object.freeze([...records]),
						complete: target.complete ?? false,
						metadata
					})
				})
			);
		},
		deleteIndex(target: ReplicaIndexTarget): void {
			assertActive();
			operations.push(
				Object.freeze({
					kind: 'delete-index' as const,
					key: indexKeyFromTarget(target)
				})
			);
		}
	});
	try {
		const result = update(writer);
		assertReplicaOptimisticUpdateSynchronous(result);
	} finally {
		active = false;
	}
	changes.push(...suppliedChanges);
	return Object.freeze({
		operations: Object.freeze(operations),
		context: cloneJsonValue({
			id,
			changes
		}) as CacheValue
	});
}

function replayReplicaOptimisticUpdate(
	writer: OptimisticCacheWriter,
	operations: readonly CapturedReplicaOptimisticOperation[]
): void {
	for (const operation of operations) {
		if (operation.kind === 'write-record') {
			writer.writeRecord(operation.write);
		} else if (operation.kind === 'tombstone-record') {
			writer.tombstoneRecord(operation.key);
		} else if (operation.kind === 'write-index') {
			writer.writeIndex(operation.write);
		} else {
			writer.deleteIndex(operation.key);
		}
	}
}

function cloneOptimisticFields(
	fields: ReplicaRecordPatch['fields']
): Readonly<Record<string, CacheValue>> {
	if (fields === undefined) return Object.freeze({});
	assertPlainReplicaRecord(fields, 'record fields');
	return Object.freeze(
		Object.fromEntries(
			Object.entries(fields).map(([name, value]) => {
				assertReplicaName(name, 'record field');
				return [name, cloneJsonValue(value) as CacheValue];
			})
		)
	);
}

function cloneOptimisticLinks(
	links: ReplicaRecordPatch['links']
): Readonly<Record<string, RecordLink>> {
	if (links === undefined) return Object.freeze({});
	assertPlainReplicaRecord(links, 'record links');
	return Object.freeze(
		Object.fromEntries(
			Object.entries(links).map(([name, value]) => {
				assertReplicaName(name, 'record link');
				if (value === null) return [name, null];
				if (typeof value === 'string') {
					assertReplicaName(value, 'record key');
					return [name, value];
				}
				if (!Array.isArray(value)) {
					throw new TypeError(
						'record link must be a key, key array, or null'
					);
				}
				const keys = value.map((key) => {
					assertReplicaName(key, 'record key');
					return key;
				});
				return [name, Object.freeze(keys)];
			})
		)
	);
}

function assertPlainReplicaRecord(
	value: object,
	description: string
): void {
	const prototype = Object.getPrototypeOf(value);
	if (
		Array.isArray(value) ||
		(prototype !== Object.prototype && prototype !== null)
	) {
		throw new TypeError(`${description} must be a plain object`);
	}
}

function assertReplicaOptimisticLayerId(id: string): void {
	assertReplicaName(id, 'optimistic layer id');
}

function assertReplicaName(value: string, description: string): void {
	if (typeof value !== 'string' || value.length === 0) {
		throw new TypeError(`${description} must be a non-empty string`);
	}
}

function assertReplicaOptimisticUpdateSynchronous(result: unknown): void {
	if (
		result !== null &&
		(typeof result === 'object' || typeof result === 'function') &&
		typeof (result as { then?: unknown }).then === 'function'
	) {
		void Promise.resolve(result).catch(() => undefined);
		throw new TypeError('optimistic layer update must be synchronous');
	}
}

function indexMaintenanceSnapshot(
	confirmed: CacheEngineSnapshot
): ReplicaIndexMaintenanceSnapshot {
	return Object.freeze({
		records: Object.freeze(
			confirmed.records.flatMap((record) => {
				if (record.tombstoneRevision !== undefined) return [];
				const model = modelFromRecordKey(record.key);
				if (model === undefined) return [];
				return [
					Object.freeze({
						key: record.key,
						model,
						fields: Object.freeze(
							Object.fromEntries(
								Object.entries(record.fields).map(([name, field]) => [
									name,
									field.value
								])
							)
						)
					})
				];
			})
		),
		indexes: Object.freeze(
			confirmed.indexes.flatMap((index) => {
				if (index.deleted || index.metadata === undefined) return [];
				return [
					Object.freeze({
						key: index.key,
						records: index.records,
						complete: index.complete,
						metadata: index.metadata
					})
				];
			})
		)
	});
}

function indexSemanticLayer(
	layer: OptimisticLayerView
): ReplicaIndexSemanticLayer {
	const context = layer.context;
	if (
		context === null ||
		Array.isArray(context) ||
		typeof context !== 'object'
	) {
		throw new TypeError(
			`optimistic layer ${layer.id} has invalid index-maintenance context`
		);
	}
	const record = context as Readonly<Record<string, CacheValue>>;
	if (record.id !== layer.id || !Array.isArray(record.changes)) {
		throw new TypeError(
			`optimistic layer ${layer.id} has invalid index-maintenance context`
		);
	}
	return Object.freeze({
		id: layer.id,
		changes: record.changes as unknown as readonly ReplicaIndexSemanticChange[]
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

function replicaClientRequestExtensions<
	TData,
	TVariables extends GraphqlVariables
>(
	artifact: ReplicaOperationArtifact<TData, TVariables>
): { readonly extensions?: Readonly<Record<string, unknown>> } {
	validatedArtifactBinding(artifact);
	const protocol = artifact.protocol;
	const surface =
		protocol.surface.kind === 'role'
			? Object.freeze({
					kind: 'role' as const,
					name: protocol.surface.name
				})
			: Object.freeze({
					kind: 'application' as const,
					name: protocol.surface.name,
					roles: Object.freeze([...protocol.surface.roles])
				});
	return Object.freeze({
		extensions: Object.freeze({
			distributed: Object.freeze({
				client: Object.freeze({
					surface,
					schemaHash: protocol.schemaHash
				})
			})
		})
	});
}

function validatedCommandAuthorityContract(
	value: ReplicaCommandSurfaceContract
): RegisteredCommandAuthorityContract {
	if (
		value === null ||
		typeof value !== 'object' ||
		value.protocolVersion !== 2 ||
		typeof value.schemaHash !== 'string' ||
		!SHA256.test(value.schemaHash) ||
		typeof value.protocolHash !== 'string' ||
		!SHA256.test(value.protocolHash) ||
		!Array.isArray(value.trustedPresets)
	) {
		throw new TypeError('replica command authority contract is invalid');
	}
	const surfaceIdentity = validatedSurfaceIdentity(value.surface);
	const names = new Set<string>();
	const trustedPresets = Object.freeze(
		value.trustedPresets
			.map((descriptor) => {
				if (
					descriptor === null ||
					typeof descriptor !== 'object' ||
					typeof descriptor.name !== 'string' ||
					descriptor.name.length === 0 ||
					descriptor.name.length > 128 ||
					descriptor.name.trim() !== descriptor.name ||
					/[\u0000-\u001f\u007f-\u009f]/.test(descriptor.name) ||
					names.has(descriptor.name) ||
					!isDistributedTrustedPresetCodec(descriptor.codec)
				) {
					throw new TypeError(
						'replica command trusted preset contract is invalid'
					);
				}
				names.add(descriptor.name);
				return Object.freeze({
					name: descriptor.name,
					codec: descriptor.codec
				});
			})
			.sort(({ name: left }, { name: right }) =>
				left < right ? -1 : left > right ? 1 : 0
			)
	);
	const fingerprint = JSON.stringify([
		2,
		value.schemaHash,
		value.protocolHash,
		surfaceIdentity,
		trustedPresets
	]);
	return Object.freeze({
		schemaHash: value.schemaHash,
		protocolHash: value.protocolHash,
		surfaceIdentity,
		trustedPresets,
		fingerprint
	});
}

function canonicalTrustedPresets(
	value: readonly DistributedTrustedPreset[]
): readonly DistributedTrustedPreset[] {
	return Object.freeze(
		[...value].sort(({ name: left }, { name: right }) =>
			left < right ? -1 : left > right ? 1 : 0
		)
	);
}

function trustedPresetInventoryFingerprint(
	value: readonly DistributedTrustedPreset[]
): string {
	return JSON.stringify(canonicalTrustedPresets(value));
}

function trustedPresetDescriptorFingerprint(
	value: readonly ReplicaTrustedPresetDescriptor[]
): string {
	return JSON.stringify(value);
}

function operationKey<TData, TVariables extends GraphqlVariables>(
	artifact: ReplicaOperationArtifact<TData, TVariables>,
	variables: TVariables
): string {
	const binding = validatedArtifactBinding(artifact);
	const artifactIdentity = JSON.stringify([
		binding.version,
		binding.schemaHash,
		binding.surfaceIdentity,
		binding.operation,
		artifact.id
	]);
	return `protocol:${artifactIdentity}:${canonicalVariables(variables)}`;
}

function validatedSurfaceIdentity(
	value: NonNullable<ReplicaOperationArtifact['protocol']>['surface']
): string {
	if (
		value === null ||
		typeof value !== 'object' ||
		typeof value.name !== 'string' ||
		value.name.length === 0
	) {
		throw new TypeError('replica artifact client surface is invalid');
	}
	if (value.kind === 'role') {
		return JSON.stringify(['role', value.name]);
	}
	if (
		value.kind !== 'application' ||
		!Array.isArray(value.roles) ||
		value.roles.length === 0 ||
		value.roles.some(
			(role) => typeof role !== 'string' || role.length === 0
		) ||
		new Set(value.roles).size !== value.roles.length ||
		[...value.roles].sort().some((role, index) => role !== value.roles[index])
	) {
		throw new TypeError('replica artifact client surface is invalid');
	}
	return JSON.stringify(['application', value.name, value.roles]);
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
