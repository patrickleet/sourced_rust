import {
	CacheRevisionConflictError,
	createCacheEngine,
	type BaseCacheWriter,
	type CacheEngine,
	type CacheEngineSnapshot,
	type DerivedIndexMutation,
	type DerivedIndexReconciler,
	type OptimisticLayerView
} from '../../internal/cache-engine.js';
import type { GraphqlVariables } from '../../types.js';
import {
	compareDistributedDecimal,
	DistributedProtocolError,
	parseGraphqlResponseExtensions,
	type DistributedCommandMetadata,
	type DistributedLiveCursor,
	type DistributedOpaqueString,
	type DistributedProjectionObservation,
	type DistributedProtocolEnvelope,
	type DistributedQuerySnapshot,
	type DistributedRecordRevision,
	type DistributedTrustedPreset
} from '../../protocol.js';
import {
	matchReplicaTrustedPresetInventory
} from '../commands.js';
import type {
	ReplicaDiagnosticEventInput,
	ReplicaDiagnosticLayerInput,
	ReplicaDiagnosticReceiptInput,
	ReplicaDiagnosticsSink
} from '../diagnostics.js';
import {
	replicaCommandAuthority,
	replicaCommandDirectProjection,
	replicaResultObservation,
	type ReplicaCommandAuthorityRegistration,
	type ReplicaCommandAuthoritySnapshot,
	type ReplicaCommandDirectProjection,
	type ReplicaCommandSurfaceContract,
	type ReplicaResultObservationRegistration
} from '../command-runtime.js';
import {
	canonicalizeOperationVariables,
	replicaIndexKey,
	replicaRecordKey,
	resolveArguments
} from '../identity.js';
import {
	materializeReplicaOperation,
	type MaterializedReplicaResult
} from '../materialize.js';
import {
	normalizeReplicaResult,
	type ReplicaNormalizationProtocol,
	type ReplicaProtocolRecordResolution
} from '../normalize.js';
import { createReplicaRevalidationMatcher } from '../revalidation.js';
import {
	validateReplicaOperationBinding as validatedArtifactBinding
} from '../operation-binding.js';
import {
	createReplicaIndexMaintenanceRegistry,
	formatReplicaIndexStaleReason,
	type ReplicaIndexPlanRegistration,
	type ReplicaIndexSemanticChange
} from '../index-maintenance.js';
import {
	runtimeRoot,
	type RuntimeObjectBranch,
	type RuntimeObjectSelection,
	type RuntimeRootSelection
} from '../selection.js';
import type {
	DistributedReplicaOptions,
	DistributedReplica as DistributedReplicaApi,
	ReplicaAuthoritativeScope,
	ReplicaBaseWriter,
	ReplicaDehydratedState,
	ReplicaIdentity,
	ReplicaIndexInspection,
	ReplicaIndexTarget,
	ReplicaModelArtifact,
	ReplicaOperationArtifact,
	ReplicaOptimisticWriter,
	ReplicaRecordInspection,
	ReplicaRevalidationPlan,
	ReplicaRevision,
	ReplicaResultEnvelope,
	ReplicaSnapshot,
	ReplicaTransport,
	ReplicaValue,
	ReplicaWatch,
	ReplicaWriteSource,
	WatchReplicaOptions
} from '../types.js';
import {
	EMPTY_CACHE_SNAPSHOT,
	EMPTY_ERRORS,
	EMPTY_TRUSTED_PRESETS,
	MAX_ANONYMOUS_RECORD_CLOCKS
} from './constants.js';
import type {
	AnonymousRecordProtocolClock,
	IndexDisposition,
	LiveEntry,
	OperationProtocolGroup,
	OperationProtocolSource,
	OperationProtocolState,
	OptimisticReceiptState,
	ProjectedRecordFence,
	ProtocolGeneration,
	QueryState,
	RecordProtocolClock,
	RegisteredCommandAuthorityContract,
	RenderedOperation,
	ReplicaArtifactBinding,
	ReplicaDehydratedPayloadV1,
	SharedIndexDisposition,
	ValidatedArtifactBinding
} from './types.js';
import {
	compareCanonicalDecimalStrings,
	compareEvidenceToProjectedFence,
	compareIndexVector,
	compareProjectedRecordFields,
	compareRecordClock,
	compareSnapshotToOperationState,
	freezeRecordClock,
	incrementCanonicalDecimal,
	indexClockMap,
	isComparableHandoffDisposition,
	latestCursors,
	modelFromRecordKey,
	protocolInvalid,
	recordKeyMatchesModel,
	responsePathKey,
	sameRecordClock,
	sameRecordRevision
} from './clocks.js';
import {
	hydrationMetadataConsistent,
	parseAuthoritativeScope,
	parseReplicaHydration,
	serializeOperationProtocolGroup
} from './hydration.js';
import {
	assertReplicaOptimisticLayerId,
	captureReplicaOptimisticUpdate,
	cloneOptimisticReceipt,
	diagnosticReceiptCounts,
	diagnosticReceiptExpectations,
	expectationKey,
	optimisticReceiptState,
	replayReplicaOptimisticUpdate,
	sameReceipt
} from './optimistic.js';
import {
	assertWriteSource,
	baseWriter,
	canonicalTrustedPresets,
	graphqlError,
	indexKeyFromTarget,
	indexMaintenanceSnapshot,
	indexSemanticLayer,
	operationKey,
	prepareRecordEvidence,
	protocolOperationSource,
	replicaClientRequestExtensions,
	replicaResultIndexKeys,
	reportSafely,
	reportUnhandledObserverError,
	snapshotFrom,
	stableErrors,
	trustedPresetDescriptorFingerprint,
	trustedPresetInventoryFingerprint,
	validatedCommandAuthorityContract
} from './helpers.js';
import { ReplicaWatchState } from './watch.js';

export class DistributedReplicaImpl implements DistributedReplicaApi {
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
	readonly #projectedRecordFences = new Map<string, ProjectedRecordFence>();
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
	#projectionGeneration = 0;
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

	[replicaCommandDirectProjection](
		commandId: string,
		projection: ReplicaCommandDirectProjection
	): void {
		const { model, identity, evidence, fields } = projection;
		if (evidence.model !== model.id || evidence.tombstone) {
			protocolInvalid('extensions.distributed.command.records');
		}
		const recordKey = replicaRecordKey(model, identity);
		const pendingRecordClocks = new Map<string, RecordProtocolClock>();
		const pendingRecordScopes = new Map<DistributedOpaqueString, string>();
		const pendingAnonymousRecordClocks = new Map<
			DistributedOpaqueString,
			AnonymousRecordProtocolClock
		>();
		const consumedAnonymousRecordClocks = new Set<DistributedOpaqueString>();
		const apply = this.#resolveRecordEvidence(
			recordKey,
			evidence,
			pendingRecordClocks,
			pendingRecordScopes,
			pendingAnonymousRecordClocks,
			consumedAnonymousRecordClocks
		);

		this.confirmOptimisticLayer(commandId, (writer) => {
			if (!apply) return false;
			return writer.writeRecord(model, identity, evidence.revision, {
				incarnation: evidence.incarnation,
				fields
			});
		});

		for (const [key, clock] of pendingRecordClocks) {
			this.#recordClocks.set(key, clock);
			this.#recordKeysByScope.set(clock.scopeToken, key);
		}
		for (const [scopeToken, key] of pendingRecordScopes) {
			this.#recordKeysByScope.set(scopeToken, key);
		}
		for (const [scopeToken, clock] of pendingAnonymousRecordClocks) {
			if (!consumedAnonymousRecordClocks.has(scopeToken)) {
				this.#anonymousRecordClocks.set(scopeToken, clock);
			}
		}
		for (const scopeToken of consumedAnonymousRecordClocks) {
			this.#anonymousRecordClocks.delete(scopeToken);
		}
		if (apply) {
			this.#projectionGeneration += 1;
			/*
			 * The command result is authoritative before an asynchronous read
			 * model necessarily catches up. Retain its complete row and causal
			 * clock as a write fence until a query acknowledges it or a newer
			 * record/tombstone supersedes it.
			 */
			this.#projectedRecordFences.set(
				recordKey,
				Object.freeze({
					fields: Object.freeze({ ...fields }),
					clock: Object.freeze({
						scopeToken: evidence.scopeToken,
						incarnation: evidence.incarnation,
						revision: evidence.revision,
						tombstone: false
					}),
					projectionGeneration: this.#projectionGeneration
				})
			);
		}
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
		requestRevision?: string,
		responseProjectionGeneration?: number
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
			requestRevision,
			responseProjectionGeneration
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
		requestRevision: string | undefined,
		responseProjectionGeneration: number | undefined
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
		const pendingProjectedRecordFenceClears = new Map<
			string,
			ProjectedRecordFence
		>();
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
				recordKey,
				fields
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
				const projectedFence =
					this.#projectedRecordFences.get(recordKey);
				const projectedDisposition =
					projectedFence === undefined
						? undefined
						: compareProjectedRecordFields(
								projectedFence.fields,
								fields
							);
				const projectedClockComparison =
					projectedFence === undefined
						? undefined
						: compareEvidenceToProjectedFence(
								evidence,
								projectedFence
							);
				const newerPostProjectionRow =
					projectedFence !== undefined &&
					projectedDisposition === 'conflict' &&
					projectedClockComparison === 1 &&
					responseProjectionGeneration !== undefined &&
					responseProjectionGeneration >=
						projectedFence.projectionGeneration;
				/*
				 * Snapshot bodies can race: SQL may be read before a command
				 * while response evidence is stamped after it. Conflicting
				 * pre-command responses therefore cannot override a projected
				 * fence, even when their evidence revision is numerically later.
				 * A causally newer row from an HTTP request or live frame that
				 * began after the command is an atomic supersession and may
				 * release the fence without first echoing the projected body.
				 */
				if (projectedFence !== undefined) {
					const exactEcho =
						projectedDisposition === 'complete' &&
						(
							projectedClockComparison === 0 ||
							projectedClockComparison === 1
						);
					const newerPartial =
						projectedDisposition === 'partial' &&
						projectedClockComparison === 1;
					if (exactEcho || newerPartial || newerPostProjectionRow) {
						pendingProjectedRecordFenceClears.set(
							recordKey,
							projectedFence
						);
					}
				}
				const resolution = this.#resolveRecordEvidence(
					recordKey,
					evidence,
					pendingRecordClocks,
					pendingRecordScopes,
					pendingAnonymousRecordClocks,
					consumedAnonymousRecordClocks,
					projectedDisposition === 'conflict' &&
						!newerPostProjectionRow
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
					consumedRecordPaths,
					pendingProjectedRecordFenceClears
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
					consumedAnonymousRecordClocks,
					pendingProjectedRecordFenceClears
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
		for (const [recordKey, projectedFields] of pendingProjectedRecordFenceClears) {
			if (this.#projectedRecordFences.get(recordKey) === projectedFields) {
				this.#projectedRecordFences.delete(recordKey);
			}
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
		this.#projectedRecordFences.clear();
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
		this.#projectionGeneration = 0;
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
		consumedAnonymousClocks: Set<DistributedOpaqueString>,
		projectedConflict = false
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
		if (projectedConflict) {
			pendingClocks.set(recordKey, current);
			return false;
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
		consumedPaths: Set<string>,
		pendingProjectedRecordFenceClears: Map<
			string,
			ProjectedRecordFence
		>
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
			const projectedFence =
				this.#projectedRecordFences.get(recordKey);
			const projectedClockComparison =
				projectedFence === undefined
					? undefined
					: compareEvidenceToProjectedFence(
							evidence,
							projectedFence
						);
			if (
				projectedFence !== undefined &&
				projectedClockComparison === 1
			) {
				pendingProjectedRecordFenceClears.set(
					recordKey,
					projectedFence
				);
			}
			if (
				this.#resolveRecordEvidence(
					recordKey,
					evidence,
					pendingClocks,
					pendingScopes,
					pendingAnonymousClocks,
					consumedAnonymousClocks,
					projectedFence !== undefined &&
						projectedClockComparison !== 1
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
		consumedAnonymousClocks: Set<DistributedOpaqueString>,
		pendingProjectedRecordFenceClears: Map<
			string,
			ProjectedRecordFence
		>
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
			const projectedFence =
				this.#projectedRecordFences.get(recordKey);
			const projectedClockComparison =
				projectedFence === undefined
					? undefined
					: compareEvidenceToProjectedFence(
							evidence,
							projectedFence
						);
			if (
				projectedFence !== undefined &&
				projectedClockComparison === 1
			) {
				pendingProjectedRecordFenceClears.set(
					recordKey,
					projectedFence
				);
			}
			if (
				this.#resolveRecordEvidence(
					recordKey,
					evidence,
					pendingClocks,
					pendingScopes,
					pendingAnonymousClocks,
					consumedAnonymousClocks,
					projectedFence !== undefined &&
						projectedClockComparison !== 1
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
		const projectionGeneration = this.#projectionGeneration;
		/*
		 * Reserve local index ordering and the projection-fence generation when
		 * the request starts, not when it finishes. Distinct operation artifacts
		 * may share the same semantic index key; a slower earlier request must
		 * not replace a later-started result merely because its response arrived
		 * last, nor may it supersede a direct projection installed in between.
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
					requestRevision,
					projectionGeneration
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
						// A live frame begins at this synchronous ingress boundary.
						const projectionGeneration = this.#projectionGeneration;
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
								'live',
								undefined,
								projectionGeneration
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
