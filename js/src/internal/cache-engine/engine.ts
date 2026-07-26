import { CacheRevisionConflictError } from './errors.js';
import {
	assertName,
	assertSynchronousResult,
	assertWriterActive,
	cloneCacheValue,
	cloneDerivedIndexOperations,
	cloneFields,
	cloneIndexMetadata,
	cloneLink,
	cloneLinks,
	compareRecordTuple,
	deepEqual,
	dependenciesChanged,
	derivedIndexKeys,
	emptyVisibleRecord,
	freezeRecord,
	indexDependency,
	indexMetadataWithoutStaleReason,
	isOrderedSubsequence,
	isVisibleRecordLive,
	linkKeys,
	operationDependencies,
	parseSnapshot,
	recordFieldDependency,
	recordSeenDependency,
	recordWildcardDependency,
	refinementMetadataCompatible,
	reportSafely,
	reportUnhandledWatcherError,
	revisionString,
	revisionToken,
	runDerivedIndexReconciler,
	validateIndexWrite,
	validateRecordKey
} from './helpers.js';
import type {
	BaseCacheWriter,
	CacheEngine,
	CacheEngineOptions,
	CacheEngineSnapshot,
	CacheIndex,
	CacheListener,
	CachePresence,
	CacheReader,
	CacheSelector,
	CacheValue,
	DerivedIndexOperation,
	DerivedIndexReconciler,
	EngineBackup,
	IndexKey,
	IndexWrite,
	MaterializedCacheGraph,
	OptimisticCacheWriter,
	OptimisticIndexWrite,
	OptimisticLayer,
	OptimisticLayerContext,
	OptimisticLayerState,
	OptimisticRecordWrite,
	OverlayOperation,
	RecordKey,
	RecordLink,
	RecordWrite,
	Revision,
	SparseRecord,
	SparseRecordMeta,
	StoredIndex,
	StoredRecord,
	VisibleIndex,
	VisibleRecord,
	WatchOptions,
	Watcher
} from './types.js';

/**
 * Minimum purpose-built implementation selected by the executable spike.
 *
 * It stores authoritative sparse records and exact indexes, then materializes
 * named optimistic operation layers above them. Confirmation advances a
 * per-dependency causal floor before removing the layer, so an older pending
 * layer cannot become visible again after a newer command confirms.
 */
export class PurposeBuiltCacheEngine implements CacheEngine {
	#records = new Map<RecordKey, StoredRecord>();
	#indexes = new Map<IndexKey, StoredIndex>();
	#layers: OptimisticLayer[] = [];
	#derivedIndexOperations: DerivedIndexOperation[] = [];
	#derivedIndexReconciler: DerivedIndexReconciler | undefined;
	#reconcilingDerivedIndexes = false;
	#confirmedFloors = new Map<string, number>();
	#retained = new Map<RecordKey, number>();
	#nextLayerSequence = 0;
	#watchers = new Set<Watcher>();
	#transactionDepth = 0;
	#transactionLifecycleIndexes = new Set<IndexKey>();
	#dirty = false;
	#changedDependencies = new Set<string>();
	readonly #reportWatcherError: (error: AggregateError) => void;

	constructor(options: CacheEngineOptions = {}) {
		this.#reportWatcherError = options.onWatcherError ?? reportUnhandledWatcherError;
	}

	read<T>(selector: CacheSelector<T>): T {
		if (typeof selector !== 'function') throw new TypeError('cache selector must be a function');
		return selector(this.#reader());
	}

	readConfirmed<T>(selector: CacheSelector<T>): T {
		if (typeof selector !== 'function') throw new TypeError('cache selector must be a function');
		return selector(this.#reader(undefined, false));
	}

	confirmedIndexFences(
		keys: readonly IndexKey[]
	): ReadonlyMap<IndexKey, string> {
		const fences = new Map<IndexKey, string>();
		for (const key of new Set(keys)) {
			assertName(key, 'index key');
			const index = this.#indexes.get(key);
			if (index === undefined) continue;
			const fence =
				index.staleRevision !== undefined &&
				index.staleRevision > index.revision
					? index.staleRevision
					: index.revision;
			fences.set(key, revisionString(fence));
		}
		return fences;
	}

	watch<T>(
		selector: CacheSelector<T>,
		listener: CacheListener<T>,
		options: WatchOptions = {}
	): () => void {
		if (typeof listener !== 'function') throw new TypeError('cache listener must be a function');
		const { value, dependencies } = this.#select(selector);
		const watcher: Watcher<T> = { selector, listener, value, dependencies };
		this.#watchers.add(watcher as Watcher);
		if (options.immediate) {
			try {
				listener(value, undefined);
			} catch (error) {
				this.#watchers.delete(watcher as Watcher);
				reportSafely(
					this.#reportWatcherError,
					new AggregateError([error], 'initial cache watcher delivery failed')
				);
			}
		}
		return () => this.#watchers.delete(watcher as Watcher);
	}

	batch<T>(update: (writer: BaseCacheWriter) => T): T {
		if (typeof update !== 'function') throw new TypeError('cache update must be a function');
		if (this.#transactionDepth !== 0) {
			throw new Error('nested cache batches are not supported');
		}
		return this.#transaction(() => {
			const result = this.#runBaseUpdate(update);
			this.#reconcileDerivedIndexes();
			return result;
		});
	}

	createOptimisticLayer(
		id: string,
		update: (writer: OptimisticCacheWriter) => void,
		context?: OptimisticLayerContext
	): void {
		assertName(id, 'optimistic layer id');
		if (this.#layers.some((layer) => layer.id === id)) {
			throw new Error(`optimistic layer already exists: ${id}`);
		}

		if (typeof update !== 'function') {
			throw new TypeError('optimistic layer update must be a function');
		}
		const operations: OverlayOperation[] = [];
		this.#runOptimisticUpdate(update, operations);
		const stableContext =
			context === undefined ? undefined : cloneCacheValue(context);
		this.#transaction(() => {
			const before = this.#materialize();
			this.#layers.push({
				id,
				sequence: ++this.#nextLayerSequence,
				state: 'optimistic',
				operations,
				...(stableContext === undefined ? {} : { context: stableContext })
			});
			this.#markOverlayChanges(operations, before, this.#materialize());
			this.#dirty = true;
			this.#reconcileDerivedIndexes();
		});
	}

	setDerivedIndexReconciler(
		reconciler: DerivedIndexReconciler | undefined
	): void {
		if (reconciler !== undefined && typeof reconciler !== 'function') {
			throw new TypeError('derived index reconciler must be a function');
		}
		const previous = this.#derivedIndexReconciler;
		try {
			this.#transaction(() => {
				this.#derivedIndexReconciler = reconciler;
				this.#reconcileDerivedIndexes();
			});
		} catch (error) {
			this.#derivedIndexReconciler = previous;
			throw error;
		}
	}

	markOptimisticLayerAccepted(id: string): boolean {
		const layer = this.#layers.find((candidate) => candidate.id === id);
		if (!layer) return false;
		this.#transaction(() => {
			layer.state = 'accepted';
			this.#reconcileDerivedIndexes();
		});
		return true;
	}

	confirmOptimisticLayer<T>(id: string, update: (writer: BaseCacheWriter) => T): T {
		return this.confirmOptimisticLayers([id], update);
	}

	confirmOptimisticLayers<T>(
		ids: readonly string[],
		update: (writer: BaseCacheWriter) => T
	): T {
		const unique = [...new Set(ids)];
		if (unique.length !== ids.length) {
			throw new Error('optimistic layer confirmation contains duplicate ids');
		}
		const layers = unique.map((id) => {
			const layer = this.#layers.find((candidate) => candidate.id === id);
			if (!layer) throw new Error(`unknown optimistic layer: ${id}`);
			return layer;
		});

		return this.#transaction(() => {
			const before = this.#materialize();
			const layerDependencies = new Map<string, number>();
			for (const layer of layers) {
				for (const operation of layer.operations) {
					for (const dependency of operationDependencies(operation)) {
						layerDependencies.set(
							dependency,
							Math.max(layerDependencies.get(dependency) ?? 0, layer.sequence)
						);
					}
				}
			}

			// A command result may contain a full row, but only fields owned by this
			// optimistic layer are confirmed. Advancing floors for every field in the
			// server payload would incorrectly erase independent, older overlays.
			const result = this.#runBaseUpdate(update);
			for (const [dependency, sequence] of layerDependencies) {
				const previous = this.#confirmedFloors.get(dependency) ?? 0;
				if (sequence > previous) this.#confirmedFloors.set(dependency, sequence);
			}
			const removed = new Set(layers);
			this.#layers = this.#layers.filter((candidate) => !removed.has(candidate));
			for (const layer of layers) {
				this.#markOverlayChanges(layer.operations, before, this.#materialize());
			}
			if (this.#layers.length === 0) this.#confirmedFloors.clear();
			this.#dirty = true;
			this.#reconcileDerivedIndexes();
			return result;
		});
	}

	rejectOptimisticLayer(id: string): boolean {
		const layer = this.#layers.find((candidate) => candidate.id === id);
		if (!layer) return false;
		this.#transaction(() => {
			const before = this.#materialize();
			this.#layers = this.#layers.filter((candidate) => candidate !== layer);
			this.#markOverlayChanges(layer.operations, before, this.#materialize());
			if (this.#layers.length === 0) this.#confirmedFloors.clear();
			this.#dirty = true;
			this.#reconcileDerivedIndexes();
		});
		return true;
	}

	optimisticLayerState(id: string): OptimisticLayerState | undefined {
		return this.#layers.find((layer) => layer.id === id)?.state;
	}

	extract(): CacheEngineSnapshot {
		return Object.freeze({
			version: 1 as const,
			records: Object.freeze(
				[...this.#records]
					.sort(([left], [right]) => left.localeCompare(right))
					.map(([key, record]) =>
						Object.freeze({
							key,
							revision: revisionString(record.revision),
							incarnation: revisionString(record.incarnation),
							...(record.tombstoneRevision === undefined
								? {}
								: { tombstoneRevision: revisionString(record.tombstoneRevision) }),
							fields: freezeRecord(
								[...record.fields]
									.sort(([left], [right]) => left.localeCompare(right))
									.map(([name, field]) => [
										name,
										Object.freeze({
											revision: revisionString(field.revision),
											value: cloneCacheValue(field.value)
										})
									])
							),
							links: freezeRecord(
								[...record.links]
									.sort(([left], [right]) => left.localeCompare(right))
									.map(([name, link]) => [
										name,
										Object.freeze({
											revision: revisionString(link.revision),
											value: cloneLink(link.value)
										})
									])
							)
						})
					)
			),
			indexes: Object.freeze(
				[...this.#indexes]
					.sort(([left], [right]) => left.localeCompare(right))
					.map(([key, index]) =>
						Object.freeze({
							key,
							revision: revisionString(index.revision),
							...(index.staleRevision === undefined
								? {}
								: { staleRevision: revisionString(index.staleRevision) }),
							records: Object.freeze([...index.records]),
							complete: index.complete,
							deleted: index.deleted,
							...(index.metadata === undefined
								? {}
								: { metadata: cloneIndexMetadata(index.metadata) })
						})
					)
			)
		});
	}

	restore(snapshot: CacheEngineSnapshot): void {
		const restored = parseSnapshot(snapshot);
		this.#transaction(() => {
			this.#records = restored.records;
			this.#indexes = restored.indexes;
			this.#layers = [];
			this.#derivedIndexOperations = [];
			this.#confirmedFloors.clear();
			this.#nextLayerSequence = 0;
			this.#changedDependencies.add('*');
			this.#dirty = true;
		});
	}

	restoreConfirmed(snapshot: CacheEngineSnapshot): void {
		const restored = parseSnapshot(snapshot);
		this.#transaction(() => {
			this.#records = restored.records;
			this.#indexes = restored.indexes;
			this.#derivedIndexOperations = [];
			this.#changedDependencies.add('*');
			this.#dirty = true;
			this.#reconcileDerivedIndexes();
		});
	}

	discardIndexes(keys: readonly IndexKey[]): void {
		const unique = new Set<IndexKey>();
		for (const key of keys) {
			assertName(key, 'index key');
			unique.add(key);
		}
		if (unique.size === 0) return;
		this.#transaction(() => {
			let changed = false;
			for (const key of unique) {
				if (!this.#indexes.delete(key)) continue;
				this.#changedDependencies.add(indexDependency(key));
				changed = true;
			}
			if (changed) this.#dirty = true;
			this.#reconcileDerivedIndexes();
		});
	}

	retain(key: RecordKey): void {
		assertName(key, 'record key');
		this.#retained.set(key, (this.#retained.get(key) ?? 0) + 1);
	}

	release(key: RecordKey): void {
		const count = this.#retained.get(key);
		if (count === undefined) return;
		if (count <= 1) this.#retained.delete(key);
		else this.#retained.set(key, count - 1);
	}

	gc(): readonly RecordKey[] {
		return this.#transaction(() => {
			// A destructive optimistic overlay must not make confirmed state
			// collectible: rejecting that layer has to reveal the complete base
			// graph again. Conversely, an optimistic index/link may be the only
			// current root for an authoritative record. Preserve the union.
			const optimisticRoots = this.#optimisticRecordRoots();
			const baseGraph = this.#materialize(false);
			const visibleGraph = this.#materialize(true);
			const reachable = new Set<RecordKey>([
				...this.#reachableRecords(baseGraph, optimisticRoots),
				...this.#reachableRecords(visibleGraph, optimisticRoots)
			]);

			const collected: RecordKey[] = [];
			for (const [key, record] of this.#records) {
				// Tombstones are revision fences and cannot be collected like data rows.
				if (record.tombstoneRevision === undefined && !reachable.has(key)) {
					this.#records.delete(key);
					this.#changedDependencies.add(recordSeenDependency(key));
					this.#changedDependencies.add(recordWildcardDependency(key));
					collected.push(key);
				}
			}
			let indexesCollected = false;
			for (const [key, index] of this.#indexes) {
				const parent = index.metadata?.parent;
				if (parent === undefined) continue;
				const parentIsLive =
					isVisibleRecordLive(baseGraph.records.get(parent)) ||
					isVisibleRecordLive(visibleGraph.records.get(parent));
				if (reachable.has(parent) && parentIsLive) continue;
				this.#indexes.delete(key);
				this.#changedDependencies.add(indexDependency(key));
				indexesCollected = true;
			}
			if (collected.length > 0 || indexesCollected) this.#dirty = true;
			this.#reconcileDerivedIndexes();
			return Object.freeze(collected.sort());
		});
	}

	#reader(
		dependencies?: Set<string>,
		includeOptimistic = true
	): CacheReader {
		// V1 executable-spike tradeoff: materializing the visible graph is
		// O(records + indexes + overlay operations) per selector. The private seam
		// keeps this replaceable with an incrementally indexed graph without
		// changing generated artifacts or the public replica API.
		const { records, indexes } = this.#materialize(includeOptimistic);
		return Object.freeze({
			recordMeta(key: RecordKey): SparseRecordMeta | undefined {
				dependencies?.add(recordSeenDependency(key));
				const record = records.get(key);
				if (!record || record.tombstoned) return undefined;
				return Object.freeze({
					key,
					incarnation: revisionString(record.incarnation)
				});
			},
			field(key: RecordKey, name: string): CachePresence<CacheValue> {
				assertName(name, 'record field');
				dependencies?.add(recordSeenDependency(key));
				dependencies?.add(recordFieldDependency(key, `field:${name}`));
				const record = records.get(key);
				if (!record || record.tombstoned || !record.fields.has(name)) {
					return Object.freeze({ present: false });
				}
				return Object.freeze({
					present: true,
					value: cloneCacheValue(record.fields.get(name)!)
				});
			},
			link(key: RecordKey, name: string): CachePresence<RecordLink> {
				assertName(name, 'record link');
				dependencies?.add(recordSeenDependency(key));
				dependencies?.add(recordFieldDependency(key, `link:${name}`));
				const record = records.get(key);
				if (!record || record.tombstoned || !record.links.has(name)) {
					return Object.freeze({ present: false });
				}
				return Object.freeze({
					present: true,
					value: cloneLink(record.links.get(name)!)
				});
			},
			record(key: RecordKey): SparseRecord | undefined {
				dependencies?.add(recordWildcardDependency(key));
				const record = records.get(key);
				if (!record || record.tombstoned) return undefined;
				return Object.freeze({
					key,
					revision: revisionString(record.revision),
					incarnation: revisionString(record.incarnation),
					fields: freezeRecord(
						[...record.fields].map(([name, value]) => [name, cloneCacheValue(value)])
					),
					links: freezeRecord(
						[...record.links].map(([name, value]) => [name, cloneLink(value)])
					)
				});
			},
			index(key: IndexKey): CacheIndex | undefined {
				dependencies?.add(indexDependency(key));
				const index = indexes.get(key);
				if (!index || index.deleted) return undefined;
				return Object.freeze({
					key,
					revision: revisionString(index.revision),
					...(index.staleRevision === undefined
						? {}
						: { staleRevision: revisionString(index.staleRevision) }),
					records: Object.freeze([...index.records]),
					complete: index.complete,
					...(index.metadata === undefined
						? {}
						: { metadata: cloneIndexMetadata(index.metadata) })
				});
			}
		});
	}

	#select<T>(selector: CacheSelector<T>): { value: T; dependencies: Set<string> } {
		const dependencies = new Set<string>();
		return { value: selector(this.#reader(dependencies)), dependencies };
	}

	#optimisticRecordRoots(): Set<RecordKey> {
		const roots = new Set<RecordKey>();
		for (const layer of this.#layers) {
			for (const operation of layer.operations) {
				if (operation.kind === 'write-record') {
					roots.add(operation.write.key);
					for (const link of Object.values(operation.write.links ?? {})) {
						for (const key of linkKeys(link)) roots.add(key);
					}
				} else if (operation.kind === 'tombstone-record') {
					roots.add(operation.key);
				} else if (operation.kind === 'write-index') {
					for (const key of operation.write.records) roots.add(key);
				}
			}
		}
		for (const operation of this.#derivedIndexOperations) {
			if (operation.kind !== 'write-index') continue;
			for (const key of operation.write.records) roots.add(key);
		}
		return roots;
	}

	#reachableRecords(
		graph: {
			records: Map<RecordKey, VisibleRecord>;
			indexes: Map<IndexKey, VisibleIndex>;
		},
		extraRoots: ReadonlySet<RecordKey>
	): Set<RecordKey> {
		const reachable = new Set<RecordKey>([...this.#retained.keys(), ...extraRoots]);
		const pending = [...reachable];
		const relationshipIndexes = new Map<RecordKey, VisibleIndex[]>();
		for (const index of graph.indexes.values()) {
			if (index.deleted) continue;
			const parent = index.metadata?.parent;
			if (parent !== undefined) {
				const indexes = relationshipIndexes.get(parent) ?? [];
				indexes.push(index);
				relationshipIndexes.set(parent, indexes);
				continue;
			}
			for (const key of index.records) {
				if (reachable.has(key)) continue;
				reachable.add(key);
				pending.push(key);
			}
		}
		while (pending.length > 0) {
			const key = pending.pop()!;
			const record = graph.records.get(key);
			if (!record || record.tombstoned) continue;
			for (const link of record.links.values()) {
				for (const key of linkKeys(link)) {
					if (reachable.has(key)) continue;
					reachable.add(key);
				pending.push(key);
				}
			}
			for (const index of relationshipIndexes.get(key) ?? []) {
				for (const child of index.records) {
					if (reachable.has(child)) continue;
					reachable.add(child);
					pending.push(child);
				}
			}
		}
		return reachable;
	}

	#materialize(includeOptimistic = true): MaterializedCacheGraph {
		const records = new Map<RecordKey, VisibleRecord>();
		const indexes = new Map<IndexKey, VisibleIndex>();

		for (const [key, record] of this.#records) {
			records.set(key, {
				revision: record.revision,
				incarnation: record.incarnation,
				tombstoned: record.tombstoneRevision !== undefined,
				fields: new Map([...record.fields].map(([name, field]) => [name, field.value])),
				links: new Map([...record.links].map(([name, link]) => [name, link.value]))
			});
		}
		for (const [key, index] of this.#indexes) {
			indexes.set(key, {
				revision: index.revision,
				staleRevision: index.staleRevision,
				records: [...index.records],
				complete: index.complete,
				deleted: index.deleted,
				metadata: index.metadata
			});
		}

		if (includeOptimistic) {
			for (const layer of this.#layers) {
				for (const operation of layer.operations) {
					this.#applyOverlay(records, indexes, layer.sequence, operation);
				}
			}
			for (const operation of this.#derivedIndexOperations) {
				this.#applyDerivedIndexOperation(records, indexes, operation);
			}
		}

		return { records, indexes };
	}

	#markOverlayChanges(
		operations: readonly OverlayOperation[],
		before: MaterializedCacheGraph,
		after: MaterializedCacheGraph
	): void {
		for (const operation of operations) {
			if (operation.kind === 'write-record') {
				const key = operation.write.key;
				this.#changedDependencies.add(recordWildcardDependency(key));
				for (const name of Object.keys(operation.write.fields ?? {})) {
					this.#changedDependencies.add(recordFieldDependency(key, `field:${name}`));
				}
				for (const name of Object.keys(operation.write.links ?? {})) {
					this.#changedDependencies.add(recordFieldDependency(key, `link:${name}`));
				}
				const previous = before.records.get(key);
				const next = after.records.get(key);
				if (
					(previous !== undefined && !previous.tombstoned) !==
						(next !== undefined && !next.tombstoned) ||
					previous?.incarnation !== next?.incarnation
				) {
					this.#changedDependencies.add(recordSeenDependency(key));
				}
				continue;
			}
			if (operation.kind === 'tombstone-record') {
				this.#changedDependencies.add(recordSeenDependency(operation.key));
				this.#changedDependencies.add(recordWildcardDependency(operation.key));
				continue;
			}
			this.#changedDependencies.add(
				indexDependency(operation.kind === 'write-index' ? operation.write.key : operation.key)
			);
		}
	}

	#applyOverlay(
		records: Map<RecordKey, VisibleRecord>,
		indexes: Map<IndexKey, VisibleIndex>,
		sequence: number,
		operation: OverlayOperation
	): void {
		if (operation.kind === 'write-record') {
			const { key, fields = {}, links = {} } = operation.write;
			let record = records.get(key);
			let wrote = false;
			for (const [name, value] of Object.entries(fields)) {
				if (sequence <= this.#recordFieldFloor(key, `field:${name}`)) continue;
				if (!record) record = emptyVisibleRecord();
				record.fields.set(name, value);
				wrote = true;
			}
			for (const [name, value] of Object.entries(links)) {
				if (sequence <= this.#recordFieldFloor(key, `link:${name}`)) continue;
				if (!record) record = emptyVisibleRecord();
				record.links.set(name, value);
				wrote = true;
			}
			if (wrote && record) {
				record.tombstoned = false;
				records.set(key, record);
			}
			return;
		}

		if (operation.kind === 'tombstone-record') {
			if (sequence <= (this.#confirmedFloors.get(recordSeenDependency(operation.key)) ?? 0)) {
				return;
			}
			const record = records.get(operation.key) ?? emptyVisibleRecord();
			record.tombstoned = true;
			record.fields.clear();
			record.links.clear();
			records.set(operation.key, record);
			return;
		}

		const key = operation.kind === 'write-index' ? operation.write.key : operation.key;
		if (sequence <= (this.#confirmedFloors.get(indexDependency(key)) ?? 0)) return;
		if (operation.kind === 'write-index') {
			let metadata = operation.write.metadata;
			if (metadata?.parent !== undefined) {
				const parent = records.get(metadata.parent);
				if (!parent || parent.tombstoned) return;
				if (
					metadata.parentRevision !== undefined &&
					revisionToken(metadata.parentRevision) !== parent.revision
				) {
					return;
				}
				if (
					metadata.parentIncarnation !== undefined &&
					revisionToken(metadata.parentIncarnation) !== parent.incarnation
				) {
					return;
				}
				metadata = cloneIndexMetadata({
					...metadata,
					parentIncarnation: revisionString(parent.incarnation)
				});
			}
			indexes.set(key, {
				revision: indexes.get(key)?.revision ?? 0n,
				staleRevision: indexes.get(key)?.staleRevision,
				records: [...operation.write.records],
				complete: operation.write.complete ?? false,
				deleted: false,
				metadata
			});
		} else {
			const index = indexes.get(key) ?? {
				revision: 0n,
				staleRevision: undefined,
				records: [],
				complete: false,
				deleted: true,
				metadata: undefined
			};
			index.deleted = true;
			index.records = [];
			indexes.set(key, index);
		}
	}

	#applyDerivedIndexOperation(
		records: Map<RecordKey, VisibleRecord>,
		indexes: Map<IndexKey, VisibleIndex>,
		operation: DerivedIndexOperation
	): void {
		if (operation.kind === 'mark-index-stale') {
			const index = indexes.get(operation.key);
			if (!index || index.deleted) return;
			/*
			 * Staleness is a freshness claim, not structural data loss. Keep a
			 * previously complete visible index renderable while the owner
			 * revalidates it. Operations that actually remove membership or
			 * records explicitly clear `complete` in their own lifecycle path.
			 */
			if (
				index.staleRevision === undefined ||
				index.staleRevision < index.revision
			) {
				index.staleRevision = index.revision;
			}
			if (index.metadata !== undefined) {
				index.metadata = cloneIndexMetadata({
					...index.metadata,
					staleReason: operation.reason
				});
			}
			return;
		}

		const key =
			operation.kind === 'write-index' ? operation.write.key : operation.key;
		if (operation.kind === 'delete-index') {
			const index = indexes.get(key) ?? {
				revision: 0n,
				staleRevision: undefined,
				records: [],
				complete: false,
				deleted: true,
				metadata: undefined
			};
			index.deleted = true;
			index.records = [];
			indexes.set(key, index);
			return;
		}

		let metadata = operation.write.metadata;
		if (metadata?.parent !== undefined) {
			const parent = records.get(metadata.parent);
			if (!parent || parent.tombstoned) return;
			if (
				metadata.parentRevision !== undefined &&
				revisionToken(metadata.parentRevision) !== parent.revision
			) {
				return;
			}
			if (
				metadata.parentIncarnation !== undefined &&
				revisionToken(metadata.parentIncarnation) !== parent.incarnation
			) {
				return;
			}
			metadata = cloneIndexMetadata({
				...metadata,
				parentIncarnation: revisionString(parent.incarnation)
			});
		}
		indexes.set(key, {
			revision: indexes.get(key)?.revision ?? 0n,
			staleRevision: indexes.get(key)?.staleRevision,
			records: [...operation.write.records],
			complete: operation.write.complete ?? false,
			deleted: false,
			metadata
		});
	}

	#reconcileDerivedIndexes(): void {
		if (this.#reconcilingDerivedIndexes) {
			throw new Error('derived index reconciliation cannot be re-entered');
		}
		const reconciler = this.#derivedIndexReconciler;
		let next: readonly DerivedIndexOperation[];
		this.#reconcilingDerivedIndexes = true;
		try {
			next =
				reconciler === undefined
					? []
					: cloneDerivedIndexOperations(
							runDerivedIndexReconciler(
								reconciler,
								this.extract(),
								this.#layers.map((layer) =>
									Object.freeze({
										id: layer.id,
										sequence: layer.sequence,
										state: layer.state,
										...(layer.context === undefined
											? {}
											: { context: layer.context })
									})
								)
							)
						);
		} finally {
			this.#reconcilingDerivedIndexes = false;
		}
		const before = this.#materialize();
		const previous = this.#derivedIndexOperations;
		this.#derivedIndexOperations = [...next];
		const after = this.#materialize();
		let changed = false;
		for (const key of derivedIndexKeys([...previous, ...next])) {
			if (deepEqual(before.indexes.get(key), after.indexes.get(key))) continue;
			this.#changedDependencies.add(indexDependency(key));
			changed = true;
		}
		if (changed) this.#dirty = true;
	}

	#recordFieldFloor(key: RecordKey, field: string): number {
		return Math.max(
			this.#confirmedFloors.get(recordWildcardDependency(key)) ?? 0,
			this.#confirmedFloors.get(recordFieldDependency(key, field)) ?? 0
		);
	}

	#baseWriter(touched?: Set<string>, isActive: () => boolean = () => true): BaseCacheWriter {
		return Object.freeze({
			recordClock: (key: RecordKey) => {
				assertWriterActive(isActive());
				validateRecordKey(key);
				const record = this.#records.get(key);
				if (!record) return undefined;
				return Object.freeze({
					revision: revisionString(record.revision),
					incarnation: revisionString(record.incarnation),
					tombstoned: record.tombstoneRevision !== undefined
				});
			},
			writeRecord: (write: RecordWrite) => {
				assertWriterActive(isActive());
				return this.#writeBaseRecord(write, touched);
			},
			tombstoneRecord: (
				key: RecordKey,
				revision: Revision,
				incarnation?: Revision
			) => {
				assertWriterActive(isActive());
				return this.#tombstoneBaseRecord(key, revision, incarnation, touched);
			},
			discardRecord: (key: RecordKey) => {
				assertWriterActive(isActive());
				return this.#discardBaseRecord(key, touched);
			},
			writeIndex: (write: IndexWrite) => {
				assertWriterActive(isActive());
				return this.#writeBaseIndex(write, touched);
			},
			markIndexStale: (key: IndexKey, reason: string, revision?: Revision) => {
				assertWriterActive(isActive());
				return this.#markBaseIndexStale(key, reason, revision, touched);
			},
			deleteIndex: (key: IndexKey, revision: Revision) => {
				assertWriterActive(isActive());
				return this.#deleteBaseIndex(key, revision, touched);
			}
		});
	}

	#optimisticWriter(
		operations: OverlayOperation[],
		isActive: () => boolean = () => true
	): OptimisticCacheWriter {
		return Object.freeze({
			writeRecord(write: OptimisticRecordWrite): void {
				assertWriterActive(isActive());
				validateRecordKey(write.key);
				const fields = cloneFields(write.fields);
				const links = cloneLinks(write.links);
				if (Object.keys(fields).length === 0 && Object.keys(links).length === 0) return;
				operations.push({ kind: 'write-record', write: { key: write.key, fields, links } });
			},
			tombstoneRecord(key: RecordKey): void {
				assertWriterActive(isActive());
				validateRecordKey(key);
				operations.push({ kind: 'tombstone-record', key });
			},
			writeIndex(write: OptimisticIndexWrite): void {
				assertWriterActive(isActive());
				validateIndexWrite(write);
				operations.push({
					kind: 'write-index',
					write: {
						key: write.key,
						records: Object.freeze([...write.records]),
						complete: write.complete ?? false,
						...(write.metadata === undefined
							? {}
							: { metadata: cloneIndexMetadata(write.metadata) })
					}
				});
			},
			deleteIndex(key: IndexKey): void {
				assertWriterActive(isActive());
				assertName(key, 'index key');
				operations.push({ kind: 'delete-index', key });
			}
		});
	}

	#runBaseUpdate<T>(
		update: (writer: BaseCacheWriter) => T,
		touched?: Set<string>
	): T {
		let active = true;
		const writer = this.#baseWriter(touched, () => active);
		try {
			const result = update(writer);
			assertSynchronousResult(result, 'cache update');
			return result;
		} finally {
			active = false;
		}
	}

	#runOptimisticUpdate(
		update: (writer: OptimisticCacheWriter) => void,
		operations: OverlayOperation[]
	): void {
		let active = true;
		const writer = this.#optimisticWriter(operations, () => active);
		try {
			const result = update(writer);
			assertSynchronousResult(result, 'optimistic layer update');
		} finally {
			active = false;
		}
	}

	#writeBaseRecord(write: RecordWrite, touched?: Set<string>): boolean {
		validateRecordKey(write.key);
		const revision = revisionToken(write.revision);
		const requestedIncarnation =
			write.incarnation === undefined ? undefined : revisionToken(write.incarnation);
		const fields = cloneFields(write.fields);
		const links = cloneLinks(write.links);
		for (const name of Object.keys(fields)) {
			touched?.add(recordFieldDependency(write.key, `field:${name}`));
		}
		for (const name of Object.keys(links)) {
			touched?.add(recordFieldDependency(write.key, `link:${name}`));
		}
		if (Object.keys(fields).length > 0 || Object.keys(links).length > 0) {
			touched?.add(recordSeenDependency(write.key));
		}

		let record = this.#records.get(write.key);
		if (
			requestedIncarnation === undefined &&
			record?.tombstoneRevision !== undefined
		) {
			if (revision < record.revision) return false;
			if (revision === record.revision) {
				throw new CacheRevisionConflictError(
					recordSeenDependency(write.key),
					revision
				);
			}
		}
		const incarnation =
			requestedIncarnation ??
			(record === undefined
				? revision
				: record.tombstoneRevision === undefined
					? record.incarnation
					: revision);
		if (record !== undefined) {
			const comparison = compareRecordTuple(
				incarnation,
				revision,
				record.incarnation,
				record.revision
			);
			if (comparison < 0) return false;
			if (comparison === 0 && record.tombstoneRevision !== undefined) {
				throw new CacheRevisionConflictError(
					recordSeenDependency(write.key),
					revision
				);
			}
			if (
				comparison > 0 &&
				record.tombstoneRevision !== undefined &&
				incarnation === record.incarnation
			) {
				throw new CacheRevisionConflictError(
					recordSeenDependency(write.key),
					revision
				);
			}
		}

		let changed = false;
		let presenceChanged = false;
		if (!record) {
			this.#invalidateIndexesForRecordLifecycle(write.key, touched);
			record = {
				revision,
				incarnation,
				fields: new Map(),
				links: new Map()
			};
			this.#records.set(write.key, record);
			changed = true;
			presenceChanged = true;
		} else if (incarnation > record.incarnation) {
			this.#invalidateIndexesForRecordLifecycle(write.key, touched);
			record.fields.clear();
			record.links.clear();
			record.tombstoneRevision = undefined;
			record.incarnation = incarnation;
			record.revision = revision;
			changed = true;
			presenceChanged = true;
		}

		for (const [name, value] of Object.entries(fields)) {
			const current = record.fields.get(name);
			if (current && revision < current.revision) continue;
			if (current && revision === current.revision) {
				if (deepEqual(current.value, value)) continue;
				throw new CacheRevisionConflictError(
					recordFieldDependency(write.key, `field:${name}`),
					revision
				);
			}
			record.fields.set(name, { revision, value });
			changed = true;
		}
		for (const [name, value] of Object.entries(links)) {
			const current = record.links.get(name);
			if (current && revision < current.revision) continue;
			if (current && revision === current.revision) {
				if (deepEqual(current.value, value)) continue;
				throw new CacheRevisionConflictError(
					recordFieldDependency(write.key, `link:${name}`),
					revision
				);
			}
			record.links.set(name, { revision, value });
			changed = true;
		}
		if (incarnation === record.incarnation && revision > record.revision) {
			record.revision = revision;
			changed = true;
		}
		if (changed) {
			this.#changedDependencies.add(recordWildcardDependency(write.key));
			if (presenceChanged) this.#changedDependencies.add(recordSeenDependency(write.key));
			for (const name of Object.keys(fields)) {
				this.#changedDependencies.add(
					recordFieldDependency(write.key, `field:${name}`)
				);
			}
			for (const name of Object.keys(links)) {
				this.#changedDependencies.add(
					recordFieldDependency(write.key, `link:${name}`)
				);
			}
			this.#dirty = true;
		}
		return changed;
	}

	#tombstoneBaseRecord(
		key: RecordKey,
		revisionValue: Revision,
		incarnationValue?: Revision,
		touched?: Set<string>
	): boolean {
		validateRecordKey(key);
		const revision = revisionToken(revisionValue);
		touched?.add(recordWildcardDependency(key));
		touched?.add(recordSeenDependency(key));
		const record = this.#records.get(key);
		const incarnation =
			incarnationValue === undefined
				? (record?.incarnation ?? revision)
				: revisionToken(incarnationValue);
		if (record) {
			const comparison = compareRecordTuple(
				incarnation,
				revision,
				record.incarnation,
				record.revision
			);
			if (comparison < 0) return false;
			if (comparison === 0) {
				if (record.tombstoneRevision !== undefined) return false;
				throw new CacheRevisionConflictError(recordSeenDependency(key), revision);
			}
			record.incarnation = incarnation;
			record.revision = revision;
			record.tombstoneRevision = revision;
			record.fields.clear();
			record.links.clear();
		} else {
			this.#records.set(key, {
				revision,
				incarnation,
				tombstoneRevision: revision,
				fields: new Map(),
				links: new Map()
			});
		}
		this.#invalidateIndexesForRecordLifecycle(key, touched);
		this.#changedDependencies.add(recordSeenDependency(key));
		this.#changedDependencies.add(recordWildcardDependency(key));
		this.#dirty = true;
		return true;
	}

	#discardBaseRecord(key: RecordKey, touched?: Set<string>): boolean {
		validateRecordKey(key);
		const hadRecord = this.#records.has(key);
		const hasIndexReference = [...this.#indexes.values()].some(
			(index) =>
				!index.deleted &&
				(index.metadata?.parent === key || index.records.includes(key))
		);
		if (!hadRecord && !hasIndexReference) return false;
		this.#records.delete(key);
		touched?.add(recordSeenDependency(key));
		touched?.add(recordWildcardDependency(key));
		this.#invalidateIndexesForRecordLifecycle(key, touched);
		this.#changedDependencies.add(recordSeenDependency(key));
		this.#changedDependencies.add(recordWildcardDependency(key));
		this.#dirty = true;
		return true;
	}

	#invalidateIndexesForRecordLifecycle(
		recordKey: RecordKey,
		touched?: Set<string>
	): void {
		for (const [key, index] of this.#indexes) {
			if (index.deleted) continue;
			const ownedByRecord = index.metadata?.parent === recordKey;
			const referencesRecord = index.records.includes(recordKey);
			if (!ownedByRecord && !referencesRecord) continue;
			this.#transactionLifecycleIndexes.add(key);
			index.deleted = ownedByRecord;
			index.records = ownedByRecord
				? []
				: index.records.filter((candidate) => candidate !== recordKey);
			index.complete = false;
			if (index.metadata !== undefined) {
				index.metadata = cloneIndexMetadata({
					...index.metadata,
					staleReason: 'record-lifecycle-changed'
				});
			}
			if (index.staleRevision === undefined || index.staleRevision < index.revision) {
				index.staleRevision = index.revision;
			}
			touched?.add(indexDependency(key));
			this.#changedDependencies.add(indexDependency(key));
			this.#dirty = true;
		}
	}

	#writeBaseIndex(write: IndexWrite, touched?: Set<string>): boolean {
		validateIndexWrite(write);
		const revision = revisionToken(write.revision);
		touched?.add(indexDependency(write.key));
		const current = this.#indexes.get(write.key);
		if (current?.staleRevision !== undefined && revision < current.staleRevision) {
			return false;
		}
		if (current && revision < current.revision) return false;
		const records = [...write.records];
		const complete = write.complete ?? false;
		let metadata =
			write.metadata === undefined ? undefined : cloneIndexMetadata(write.metadata);
		if (metadata?.parent !== undefined) {
			const parent = this.#records.get(metadata.parent);
			if (!parent || parent.tombstoneRevision !== undefined) return false;
			if (
				metadata.parentRevision !== undefined &&
				revisionToken(metadata.parentRevision) !== parent.revision
			) {
				throw new CacheRevisionConflictError(
					recordSeenDependency(metadata.parent),
					revisionToken(metadata.parentRevision)
				);
			}
			if (
				metadata.parentIncarnation !== undefined &&
				revisionToken(metadata.parentIncarnation) !== parent.incarnation
			) {
				throw new CacheRevisionConflictError(
					recordSeenDependency(metadata.parent),
					revisionToken(metadata.parentIncarnation)
				);
			}
			metadata = cloneIndexMetadata({
				...metadata,
				parentIncarnation: revisionString(parent.incarnation)
			});
		}
		const staleRevision = metadata?.staleReason === undefined ? undefined : revision;
		if (current && revision === current.revision) {
			if (
				current.deleted &&
				current.metadata === undefined &&
				current.records.length === 0 &&
				current.staleRevision === revision
			) {
				// A hidden fence uses an empty deleted index so it materializes no
				// membership. Revision zero is valid, so an equal-checkpoint success
				// must be able to replace that sentinel state.
				current.records = records;
				current.complete = complete;
				current.deleted = false;
				current.metadata = metadata;
				current.staleRevision = staleRevision;
				this.#changedDependencies.add(indexDependency(write.key));
				this.#dirty = true;
				return true;
			}
			if (
				complete &&
				metadata?.staleReason === undefined &&
				(this.#transactionLifecycleIndexes.has(write.key) ||
					(!current.deleted &&
						!current.complete &&
						current.metadata?.staleReason !== 'record-lifecycle-changed' &&
						isOrderedSubsequence(current.records, records) &&
						refinementMetadataCompatible(current.metadata, metadata)))
			) {
				// A partial GraphQL result may be retried without the underlying read
				// model advancing. Permit only the monotonic incomplete -> complete
				// refinement; an already-authoritative membership still conflicts on
				// any same-revision disagreement.
				current.records = records;
				current.complete = true;
				current.deleted = false;
				current.metadata = metadata;
				current.staleRevision = undefined;
				this.#changedDependencies.add(indexDependency(write.key));
				this.#dirty = true;
				return true;
			}
			const currentComparable = indexMetadataWithoutStaleReason(current.metadata);
			const nextComparable = indexMetadataWithoutStaleReason(metadata);
			if (
				!current.deleted &&
				current.complete === complete &&
				deepEqual(current.records, records) &&
				deepEqual(currentComparable, nextComparable)
			) {
				if (
					deepEqual(current.metadata, metadata) &&
					current.staleRevision === staleRevision
				) {
					return false;
				}
				current.metadata = metadata;
				current.staleRevision = staleRevision;
				this.#changedDependencies.add(indexDependency(write.key));
				this.#dirty = true;
				return true;
			}
			throw new CacheRevisionConflictError(indexDependency(write.key), revision);
		}
		this.#indexes.set(write.key, {
			revision,
			records,
			complete,
			deleted: false,
			metadata,
			staleRevision
		});
		this.#changedDependencies.add(indexDependency(write.key));
		this.#dirty = true;
		return true;
	}

	#markBaseIndexStale(
		key: IndexKey,
		reason: string,
		revisionValue?: Revision,
		touched?: Set<string>
	): boolean {
		assertName(key, 'index key');
		assertName(reason, 'index stale reason');
		let current = this.#indexes.get(key);
		if (!current) {
			if (revisionValue === undefined) return false;
			const revision = revisionToken(revisionValue);
			this.#indexes.set(key, {
				revision: 0n,
				staleRevision: revision,
				records: [],
				complete: false,
				deleted: true,
				metadata: undefined
			});
			touched?.add(indexDependency(key));
			this.#changedDependencies.add(indexDependency(key));
			this.#dirty = true;
			return true;
		}
		if (current.deleted && revisionValue === undefined) return false;
		const revision =
			revisionValue === undefined ? current.revision : revisionToken(revisionValue);
		const fence =
			current.staleRevision !== undefined && current.staleRevision > current.revision
				? current.staleRevision
				: current.revision;
		if (revision < fence) return false;
		if (
			current.staleRevision === revision &&
			(current.metadata === undefined || current.metadata.staleReason === reason)
		) {
			return false;
		}
		touched?.add(indexDependency(key));
		if (current.metadata !== undefined) {
			current.metadata = cloneIndexMetadata({ ...current.metadata, staleReason: reason });
		}
		current.staleRevision = revision;
		this.#changedDependencies.add(indexDependency(key));
		this.#dirty = true;
		return true;
	}

	#deleteBaseIndex(key: IndexKey, revisionValue: Revision, touched?: Set<string>): boolean {
		assertName(key, 'index key');
		const revision = revisionToken(revisionValue);
		touched?.add(indexDependency(key));
		const current = this.#indexes.get(key);
		if (current?.staleRevision !== undefined && revision < current.staleRevision) {
			return false;
		}
		if (current && revision < current.revision) return false;
		if (current && revision === current.revision) {
			if (current.deleted) return false;
			throw new CacheRevisionConflictError(indexDependency(key), revision);
		}
		this.#indexes.set(key, {
			revision,
			records: [],
			complete: false,
			deleted: true,
			metadata: current?.metadata,
			staleRevision: undefined
		});
		this.#changedDependencies.add(indexDependency(key));
		this.#dirty = true;
		return true;
	}

	#transaction<T>(update: () => T): T {
		const outermost = this.#transactionDepth === 0;
		const backup = outermost ? this.#backup() : undefined;
		if (outermost) this.#transactionLifecycleIndexes = new Set();
		this.#transactionDepth += 1;
		let result: T;
		try {
			result = update();
		} catch (error) {
			this.#transactionDepth -= 1;
			if (outermost && backup) {
				this.#restoreBackup(backup);
				this.#dirty = false;
				this.#transactionLifecycleIndexes = new Set();
			}
			throw error;
		}
		this.#transactionDepth -= 1;
		if (outermost) {
			const shouldFlush = this.#dirty;
			const changedDependencies = this.#changedDependencies;
			this.#dirty = false;
			this.#changedDependencies = new Set();
			this.#transactionLifecycleIndexes = new Set();
			if (shouldFlush) this.#flushWatchers(changedDependencies);
		}
		return result;
	}

	#flushWatchers(changedDependencies: ReadonlySet<string>): void {
		const errors: unknown[] = [];
		for (const watcher of this.#watchers) {
			if (!dependenciesChanged(watcher.dependencies, changedDependencies)) continue;
			try {
				const { value: next, dependencies } = this.#select(watcher.selector);
				watcher.dependencies = dependencies;
				if (deepEqual(next, watcher.value)) continue;
				const previous = watcher.value;
				watcher.value = next;
				watcher.listener(next, previous);
			} catch (error) {
				errors.push(error);
			}
		}
		if (errors.length > 0) {
			reportSafely(
				this.#reportWatcherError,
				new AggregateError(
					errors,
					'cache transaction committed, but watcher delivery failed'
				)
			);
		}
	}

	#backup(): EngineBackup {
		const backup = structuredClone({
			records: this.#records,
			indexes: this.#indexes,
			layers: this.#layers,
			derivedIndexOperations: this.#derivedIndexOperations,
			confirmedFloors: this.#confirmedFloors,
			retained: this.#retained,
			nextLayerSequence: this.#nextLayerSequence,
			changedDependencies: new Set<string>()
		});
		backup.changedDependencies = new Set(this.#changedDependencies);
		return backup;
	}

	#restoreBackup(backup: EngineBackup): void {
		this.#records = backup.records;
		this.#indexes = backup.indexes;
		this.#layers = backup.layers;
		this.#derivedIndexOperations = backup.derivedIndexOperations;
		this.#confirmedFloors = backup.confirmedFloors;
		this.#retained = backup.retained;
		this.#nextLayerSequence = backup.nextLayerSequence;
		this.#changedDependencies = new Set(backup.changedDependencies);
	}
}
