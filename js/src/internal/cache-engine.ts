/**
 * Private normalized-cache seam used by the client-replica runtime.
 *
 * This module is deliberately absent from package exports. Generated artifacts
 * and framework adapters will depend on the CacheEngine contract through the
 * replica, never on a storage vendor or on these concrete data structures.
 */

export type Revision = number | string | bigint;
export type RecordKey = string;
export type IndexKey = string;

export type CacheValue =
	| null
	| boolean
	| number
	| string
	| readonly CacheValue[]
	| { readonly [key: string]: CacheValue };

export type RecordLink = RecordKey | readonly RecordKey[] | null;

export type RecordWrite = {
	key: RecordKey;
	revision: Revision;
	/** Stable incarnation fence. Defaults to the first live revision. */
	incarnation?: Revision;
	/** Omitted fields stay absent. A present `null` remains present. */
	fields?: Readonly<Record<string, CacheValue>>;
	/** Relationship identities are stored separately from scalar/JSON fields. */
	links?: Readonly<Record<string, RecordLink>>;
};

export type OptimisticRecordWrite = Omit<RecordWrite, 'revision' | 'incarnation'>;

export type CacheIndexCoverage =
	| { readonly kind: 'complete' }
	| { readonly kind: 'unknown' }
	| {
			readonly kind: 'offset';
			readonly offset: number;
			readonly limit?: number;
			readonly returned?: number;
			readonly hasNext?: boolean;
	  }
	| {
			readonly kind: 'cursor';
			readonly after?: CacheValue;
			readonly before?: CacheValue;
			readonly first?: number;
			readonly last?: number;
			readonly start?: CacheValue;
			readonly end?: CacheValue;
			readonly hasNext?: boolean;
			readonly hasPrevious?: boolean;
	  };

export type CacheIndexMetadata = {
	readonly parent?: RecordKey;
	/** Parent row clock claimed by the response that produced this relationship. */
	readonly parentRevision?: string;
	/** Parent lifecycle fence stamped by the engine. */
	readonly parentIncarnation?: string;
	readonly field: string;
	readonly arguments: Readonly<Record<string, CacheValue>>;
	readonly coverage: CacheIndexCoverage;
	readonly dependencies: readonly string[];
	readonly staleReason?: string;
	/** Distinguishes a present GraphQL `null` from an empty collection. */
	readonly nullValue?: boolean;
};

export type IndexWrite = {
	key: IndexKey;
	revision: Revision;
	records: readonly RecordKey[];
	complete?: boolean;
	metadata?: CacheIndexMetadata;
};

export type OptimisticIndexWrite = Omit<IndexWrite, 'revision'>;

export type SparseRecord = {
	readonly key: RecordKey;
	readonly revision: string;
	readonly incarnation: string;
	readonly fields: Readonly<Record<string, CacheValue>>;
	readonly links: Readonly<Record<string, RecordLink>>;
};

export type CacheIndex = {
	readonly key: IndexKey;
	readonly revision: string;
	readonly staleRevision?: string;
	readonly records: readonly RecordKey[];
	readonly complete: boolean;
	readonly metadata?: CacheIndexMetadata;
};

export type CachePresence<T> =
	| { readonly present: false }
	| { readonly present: true; readonly value: T };

export type SparseRecordMeta = {
	readonly key: RecordKey;
	readonly incarnation: string;
};

export type BaseRecordClock = {
	readonly revision: string;
	readonly incarnation: string;
	readonly tombstoned: boolean;
};

/** Raised when one source revision claims two different values. */
export class CacheRevisionConflictError extends Error {
	readonly dependency: string;
	readonly revision: string;

	constructor(dependency: string, revision: bigint) {
		super(`conflicting cache values at revision ${revision} for ${dependency}`);
		this.name = 'CacheRevisionConflictError';
		this.dependency = dependency;
		this.revision = revisionString(revision);
	}
}

export interface CacheReader {
	recordMeta(key: RecordKey): SparseRecordMeta | undefined;
	field(key: RecordKey, name: string): CachePresence<CacheValue>;
	link(key: RecordKey, name: string): CachePresence<RecordLink>;
	record(key: RecordKey): SparseRecord | undefined;
	index(key: IndexKey): CacheIndex | undefined;
}

export interface BaseCacheWriter {
	recordClock(key: RecordKey): BaseRecordClock | undefined;
	writeRecord(write: RecordWrite): boolean;
	tombstoneRecord(key: RecordKey, revision: Revision, incarnation?: Revision): boolean;
	/** Drop uncertified fields while preserving higher protocol clocks externally. */
	discardRecord(key: RecordKey): boolean;
	writeIndex(write: IndexWrite): boolean;
	markIndexStale(key: IndexKey, reason: string, revision?: Revision): boolean;
	deleteIndex(key: IndexKey, revision: Revision): boolean;
}

export interface OptimisticCacheWriter {
	writeRecord(write: OptimisticRecordWrite): void;
	tombstoneRecord(key: RecordKey): void;
	writeIndex(write: OptimisticIndexWrite): void;
	deleteIndex(key: IndexKey): void;
}

export type CacheSelector<T> = (reader: CacheReader) => T;
export type CacheListener<T> = (value: T, previous: T | undefined) => void;

export type WatchOptions = {
	immediate?: boolean;
};

export type CacheEngineOptions = {
	/** Observer failures are reported after every eligible watcher was delivered. */
	onWatcherError?: (error: AggregateError) => void;
};

export type OptimisticLayerState = 'optimistic' | 'accepted';

export type CacheEngineSnapshot = {
	readonly version: 1;
	readonly records: readonly {
		readonly key: RecordKey;
		readonly revision: string;
		readonly incarnation?: string;
		readonly tombstoneRevision?: string;
		readonly fields: Readonly<
			Record<string, { readonly revision: string; readonly value: CacheValue }>
		>;
		readonly links: Readonly<
			Record<string, { readonly revision: string; readonly value: RecordLink }>
		>;
	}[];
	readonly indexes: readonly {
		readonly key: IndexKey;
		readonly revision: string;
		readonly staleRevision?: string;
		readonly records: readonly RecordKey[];
		readonly complete: boolean;
		readonly deleted: boolean;
		readonly metadata?: CacheIndexMetadata;
	}[];
};

export interface CacheEngine {
	read<T>(selector: CacheSelector<T>): T;
	watch<T>(
		selector: CacheSelector<T>,
		listener: CacheListener<T>,
		options?: WatchOptions
	): () => void;
	batch<T>(update: (writer: BaseCacheWriter) => T): T;
	createOptimisticLayer(id: string, update: (writer: OptimisticCacheWriter) => void): void;
	markOptimisticLayerAccepted(id: string): boolean;
	confirmOptimisticLayer<T>(id: string, update: (writer: BaseCacheWriter) => T): T;
	confirmOptimisticLayers<T>(
		ids: readonly string[],
		update: (writer: BaseCacheWriter) => T
	): T;
	rejectOptimisticLayer(id: string): boolean;
	optimisticLayerState(id: string): OptimisticLayerState | undefined;
	extract(): CacheEngineSnapshot;
	restore(snapshot: CacheEngineSnapshot): void;
	/**
	 * Drop incomparable/reset base indexes without assigning them a fabricated
	 * revision. Pending optimistic overlays remain layered above the new gap.
	 */
	discardIndexes(keys: readonly IndexKey[]): void;
	retain(key: RecordKey): void;
	release(key: RecordKey): void;
	gc(): readonly RecordKey[];
}

type StoredField<T> = {
	revision: bigint;
	value: T;
};

type StoredRecord = {
	revision: bigint;
	incarnation: bigint;
	tombstoneRevision?: bigint;
	fields: Map<string, StoredField<CacheValue>>;
	links: Map<string, StoredField<RecordLink>>;
};

type StoredIndex = {
	revision: bigint;
	staleRevision?: bigint;
	records: RecordKey[];
	complete: boolean;
	deleted: boolean;
	metadata?: CacheIndexMetadata;
};

type OverlayOperation =
	| { kind: 'write-record'; write: OptimisticRecordWrite }
	| { kind: 'tombstone-record'; key: RecordKey }
	| { kind: 'write-index'; write: OptimisticIndexWrite }
	| { kind: 'delete-index'; key: IndexKey };

type OptimisticLayer = {
	id: string;
	sequence: number;
	state: OptimisticLayerState;
	operations: OverlayOperation[];
};

type VisibleRecord = {
	revision: bigint;
	incarnation: bigint;
	tombstoned: boolean;
	fields: Map<string, CacheValue>;
	links: Map<string, RecordLink>;
};

type VisibleIndex = {
	revision: bigint;
	staleRevision?: bigint;
	records: RecordKey[];
	complete: boolean;
	deleted: boolean;
	metadata?: CacheIndexMetadata;
};

type MaterializedCacheGraph = {
	records: Map<RecordKey, VisibleRecord>;
	indexes: Map<IndexKey, VisibleIndex>;
};

type Watcher<T = unknown> = {
	selector: CacheSelector<T>;
	listener: CacheListener<T>;
	value: T;
	dependencies: Set<string>;
};

type EngineBackup = {
	records: Map<RecordKey, StoredRecord>;
	indexes: Map<IndexKey, StoredIndex>;
	layers: OptimisticLayer[];
	confirmedFloors: Map<string, number>;
	retained: Map<RecordKey, number>;
	nextLayerSequence: number;
	changedDependencies: Set<string>;
};

/**
 * Minimum purpose-built implementation selected by the executable spike.
 *
 * It stores authoritative sparse records and exact indexes, then materializes
 * named optimistic operation layers above them. Confirmation advances a
 * per-dependency causal floor before removing the layer, so an older pending
 * layer cannot become visible again after a newer command confirms.
 */
class PurposeBuiltCacheEngine implements CacheEngine {
	#records = new Map<RecordKey, StoredRecord>();
	#indexes = new Map<IndexKey, StoredIndex>();
	#layers: OptimisticLayer[] = [];
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
		return this.#transaction(() => this.#runBaseUpdate(update));
	}

	createOptimisticLayer(
		id: string,
		update: (writer: OptimisticCacheWriter) => void
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
		this.#transaction(() => {
			const before = this.#materialize();
			this.#layers.push({
				id,
				sequence: ++this.#nextLayerSequence,
				state: 'optimistic',
				operations
			});
			this.#markOverlayChanges(operations, before, this.#materialize());
			this.#dirty = true;
		});
	}

	markOptimisticLayerAccepted(id: string): boolean {
		const layer = this.#layers.find((candidate) => candidate.id === id);
		if (!layer) return false;
		layer.state = 'accepted';
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
			this.#confirmedFloors.clear();
			this.#nextLayerSequence = 0;
			this.#changedDependencies.add('*');
			this.#dirty = true;
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
			return Object.freeze(collected.sort());
		});
	}

	#reader(dependencies?: Set<string>): CacheReader {
		// V1 executable-spike tradeoff: materializing the visible graph is
		// O(records + indexes + overlay operations) per selector. The private seam
		// keeps this replaceable with an incrementally indexed graph without
		// changing generated artifacts or the public replica API.
		const { records, indexes } = this.#materialize();
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
		this.#confirmedFloors = backup.confirmedFloors;
		this.#retained = backup.retained;
		this.#nextLayerSequence = backup.nextLayerSequence;
		this.#changedDependencies = new Set(backup.changedDependencies);
	}
}

/** Create the selected private cache-engine implementation. */
export function createCacheEngine(options: CacheEngineOptions = {}): CacheEngine {
	return new PurposeBuiltCacheEngine(options);
}

/** Canonical identity for a root or relationship index and its exact arguments. */
export function cacheIndexKey(input: {
	parent?: RecordKey;
	field: string;
	arguments?: Readonly<Record<string, CacheValue>>;
}): IndexKey {
	assertName(input.field, 'index field');
	if (input.parent !== undefined) validateRecordKey(input.parent);
	const argumentsValue = cloneCacheValue(input.arguments ?? {});
	return `${input.parent ?? '$root'}.${input.field}(${canonicalValue(argumentsValue)})`;
}

function parseSnapshot(snapshot: CacheEngineSnapshot): {
	records: Map<RecordKey, StoredRecord>;
	indexes: Map<IndexKey, StoredIndex>;
} {
	if (!snapshot || snapshot.version !== 1) throw new TypeError('unsupported cache snapshot');
	if (!Array.isArray(snapshot.records) || !Array.isArray(snapshot.indexes)) {
		throw new TypeError('invalid cache snapshot collections');
	}
	const records = new Map<RecordKey, StoredRecord>();
	for (const input of snapshot.records) {
		validateRecordKey(input.key);
		if (records.has(input.key)) throw new TypeError(`duplicate snapshot record: ${input.key}`);
		const revision = revisionToken(input.revision);
		const incarnation = revisionToken(input.incarnation ?? input.revision);
		const tombstoneRevision =
			input.tombstoneRevision === undefined
				? undefined
				: revisionToken(input.tombstoneRevision);
		if (tombstoneRevision !== undefined && tombstoneRevision !== revision) {
			throw new TypeError(`invalid tombstone revision for ${input.key}`);
		}
		const fields = new Map<string, StoredField<CacheValue>>();
		for (const [name, field] of Object.entries(input.fields) as Array<
			[string, { readonly revision: string; readonly value: CacheValue }]
		>) {
			assertName(name, 'record field');
			const fieldRevision = revisionToken(field.revision);
			if (fieldRevision > revision) throw new TypeError(`field revision exceeds ${input.key}`);
			fields.set(name, { revision: fieldRevision, value: cloneCacheValue(field.value) });
		}
		const links = new Map<string, StoredField<RecordLink>>();
		for (const [name, link] of Object.entries(input.links) as Array<
			[string, { readonly revision: string; readonly value: RecordLink }]
		>) {
			assertName(name, 'record link');
			const linkRevision = revisionToken(link.revision);
			if (linkRevision > revision) throw new TypeError(`link revision exceeds ${input.key}`);
			links.set(name, { revision: linkRevision, value: cloneLink(link.value) });
		}
		if (tombstoneRevision !== undefined && (fields.size > 0 || links.size > 0)) {
			throw new TypeError(`tombstone record contains live fields: ${input.key}`);
		}
		records.set(input.key, {
			revision,
			incarnation,
			tombstoneRevision,
			fields,
			links
		});
	}

	const indexes = new Map<IndexKey, StoredIndex>();
	for (const input of snapshot.indexes) {
		validateIndexWrite(input);
		if (indexes.has(input.key)) throw new TypeError(`duplicate snapshot index: ${input.key}`);
		validateRecordKeys(input.records);
		const revision = revisionToken(input.revision);
		const staleRevision =
			input.staleRevision === undefined
				? undefined
				: revisionToken(input.staleRevision);
		if (staleRevision !== undefined && staleRevision < revision) {
			throw new TypeError(`stale index revision precedes its snapshot: ${input.key}`);
		}
		indexes.set(input.key, {
			revision,
			staleRevision,
			records: [...input.records],
			complete: Boolean(input.complete),
			deleted: Boolean(input.deleted),
			metadata:
				input.metadata === undefined ? undefined : cloneIndexMetadata(input.metadata)
		});
	}
	return { records, indexes };
}

function operationDependencies(operation: OverlayOperation): readonly string[] {
	if (operation.kind === 'write-record') {
		return [
			recordSeenDependency(operation.write.key),
			...Object.keys(operation.write.fields ?? {}).map((name) =>
				recordFieldDependency(operation.write.key, `field:${name}`)
			),
			...Object.keys(operation.write.links ?? {}).map((name) =>
				recordFieldDependency(operation.write.key, `link:${name}`)
			)
		];
	}
	if (operation.kind === 'tombstone-record') {
		return [recordSeenDependency(operation.key), recordWildcardDependency(operation.key)];
	}
	return [indexDependency(operation.kind === 'write-index' ? operation.write.key : operation.key)];
}

function recordSeenDependency(key: RecordKey): string {
	return JSON.stringify(['record-seen', key]);
}

function recordWildcardDependency(key: RecordKey): string {
	return JSON.stringify(['record', key, '*']);
}

function recordFieldDependency(key: RecordKey, field: string): string {
	return JSON.stringify(['record', key, field]);
}

function indexDependency(key: IndexKey): string {
	return JSON.stringify(['index', key]);
}

function dependenciesChanged(
	dependencies: ReadonlySet<string>,
	changed: ReadonlySet<string>
): boolean {
	if (changed.size === 0 || changed.has('*')) return true;
	for (const dependency of dependencies) {
		if (changed.has(dependency)) return true;
	}
	return false;
}

function emptyVisibleRecord(): VisibleRecord {
	return {
		revision: 0n,
		incarnation: 0n,
		tombstoned: false,
		fields: new Map(),
		links: new Map()
	};
}

function isVisibleRecordLive(record: VisibleRecord | undefined): boolean {
	return record !== undefined && !record.tombstoned;
}

function validateRecordKey(key: RecordKey): void {
	assertName(key, 'record key');
}

function validateIndexWrite(write: {
	key: IndexKey;
	records: readonly RecordKey[];
	metadata?: CacheIndexMetadata;
}): void {
	assertName(write.key, 'index key');
	if (!Array.isArray(write.records)) throw new TypeError('index records must be an array');
	validateRecordKeys(write.records);
	if ('metadata' in write && write.metadata !== undefined) {
		const metadata = cloneIndexMetadata(write.metadata);
		const expectedKey = cacheIndexKey({
			...(metadata.parent === undefined ? {} : { parent: metadata.parent }),
			field: metadata.field,
			arguments: metadata.arguments
		});
		if (write.key !== expectedKey) {
			throw new TypeError(`index key does not match its metadata: expected ${expectedKey}`);
		}
	}
}

function indexMetadataWithoutStaleReason(
	metadata: CacheIndexMetadata | undefined
): Omit<CacheIndexMetadata, 'staleReason'> | undefined {
	if (metadata === undefined) return undefined;
	const { staleReason: _staleReason, ...rest } = metadata;
	return rest;
}

function isOrderedSubsequence(
	known: readonly RecordKey[],
	complete: readonly RecordKey[]
): boolean {
	let knownIndex = 0;
	for (const key of complete) {
		if (key === known[knownIndex]) knownIndex += 1;
	}
	return knownIndex === known.length;
}

function refinementMetadataCompatible(
	current: CacheIndexMetadata | undefined,
	next: CacheIndexMetadata | undefined
): boolean {
	if (current === undefined || next === undefined) return current === next;
	return deepEqual(refinementMetadataIdentity(current), refinementMetadataIdentity(next));
}

function refinementMetadataIdentity(metadata: CacheIndexMetadata): unknown {
	return {
		parent: metadata.parent,
		parentRevision: metadata.parentRevision,
		parentIncarnation: metadata.parentIncarnation,
		field: metadata.field,
		arguments: metadata.arguments,
		dependencies: metadata.dependencies,
		nullValue: metadata.nullValue,
		coverage: coverageRequestIdentity(metadata.coverage)
	};
}

function coverageRequestIdentity(coverage: CacheIndexCoverage): unknown {
	if (coverage.kind === 'complete' || coverage.kind === 'unknown') {
		return { kind: coverage.kind };
	}
	if (coverage.kind === 'offset') {
		return { kind: coverage.kind, offset: coverage.offset, limit: coverage.limit };
	}
	return {
		kind: coverage.kind,
		after: coverage.after,
		before: coverage.before,
		first: coverage.first,
		last: coverage.last
	};
}

function cloneIndexMetadata(metadata: CacheIndexMetadata): CacheIndexMetadata {
	if (!metadata || typeof metadata !== 'object') {
		throw new TypeError('index metadata must be an object');
	}
	assertName(metadata.field, 'index field');
	if (metadata.parent !== undefined) validateRecordKey(metadata.parent);
	if (metadata.parentRevision !== undefined) revisionToken(metadata.parentRevision);
	if (metadata.parentIncarnation !== undefined) revisionToken(metadata.parentIncarnation);
	const argumentsValue = cloneCacheValue(metadata.arguments);
	if (
		argumentsValue === null ||
		Array.isArray(argumentsValue) ||
		typeof argumentsValue !== 'object'
	) {
		throw new TypeError('index arguments must be a plain object');
	}
	if (!Array.isArray(metadata.dependencies)) {
		throw new TypeError('index dependencies must be an array');
	}
	const dependencies = [...metadata.dependencies];
	const seen = new Set<string>();
	for (const dependency of dependencies) {
		assertName(dependency, 'index dependency');
		if (seen.has(dependency)) {
			throw new TypeError(`duplicate index dependency: ${dependency}`);
		}
		seen.add(dependency);
	}
	if (metadata.staleReason !== undefined) {
		assertName(metadata.staleReason, 'index stale reason');
	}
	if (metadata.nullValue !== undefined && typeof metadata.nullValue !== 'boolean') {
		throw new TypeError('index nullValue must be a boolean');
	}

	const coverage = cloneIndexCoverage(metadata.coverage);
	return Object.freeze({
		...(metadata.parent === undefined ? {} : { parent: metadata.parent }),
		...(metadata.parentRevision === undefined
			? {}
			: { parentRevision: metadata.parentRevision }),
		...(metadata.parentIncarnation === undefined
			? {}
			: { parentIncarnation: metadata.parentIncarnation }),
		field: metadata.field,
		arguments: argumentsValue as Readonly<Record<string, CacheValue>>,
		coverage,
		dependencies: Object.freeze(dependencies),
		...(metadata.staleReason === undefined
			? {}
			: { staleReason: metadata.staleReason }),
		...(metadata.nullValue === undefined ? {} : { nullValue: metadata.nullValue })
	});
}

function cloneIndexCoverage(coverage: CacheIndexCoverage): CacheIndexCoverage {
	if (!coverage || typeof coverage !== 'object') {
		throw new TypeError('index coverage must be an object');
	}
	if (coverage.kind === 'complete' || coverage.kind === 'unknown') {
		return Object.freeze({ kind: coverage.kind });
	}
	if (coverage.kind === 'offset') {
		assertNonNegativeSafeInteger(coverage.offset, 'offset coverage offset');
		if (coverage.limit !== undefined) {
			assertNonNegativeSafeInteger(coverage.limit, 'offset coverage limit');
		}
		if (coverage.returned !== undefined) {
			assertNonNegativeSafeInteger(coverage.returned, 'offset coverage returned');
		}
		if (coverage.hasNext !== undefined && typeof coverage.hasNext !== 'boolean') {
			throw new TypeError('offset coverage hasNext must be a boolean');
		}
		return Object.freeze({
			kind: 'offset' as const,
			offset: coverage.offset,
			...(coverage.limit === undefined ? {} : { limit: coverage.limit }),
			...(coverage.returned === undefined ? {} : { returned: coverage.returned }),
			...(coverage.hasNext === undefined ? {} : { hasNext: coverage.hasNext })
		});
	}
	if (coverage.kind === 'cursor') {
		if (coverage.first !== undefined) {
			assertNonNegativeSafeInteger(coverage.first, 'cursor coverage first');
		}
		if (coverage.last !== undefined) {
			assertNonNegativeSafeInteger(coverage.last, 'cursor coverage last');
		}
		if (coverage.hasNext !== undefined && typeof coverage.hasNext !== 'boolean') {
			throw new TypeError('cursor coverage hasNext must be a boolean');
		}
		if (
			coverage.hasPrevious !== undefined &&
			typeof coverage.hasPrevious !== 'boolean'
		) {
			throw new TypeError('cursor coverage hasPrevious must be a boolean');
		}
		return Object.freeze({
			kind: 'cursor' as const,
			...(coverage.after === undefined
				? {}
				: { after: cloneCacheValue(coverage.after) }),
			...(coverage.before === undefined
				? {}
				: { before: cloneCacheValue(coverage.before) }),
			...(coverage.first === undefined ? {} : { first: coverage.first }),
			...(coverage.last === undefined ? {} : { last: coverage.last }),
			...(coverage.start === undefined
				? {}
				: { start: cloneCacheValue(coverage.start) }),
			...(coverage.end === undefined ? {} : { end: cloneCacheValue(coverage.end) }),
			...(coverage.hasNext === undefined ? {} : { hasNext: coverage.hasNext }),
			...(coverage.hasPrevious === undefined
				? {}
				: { hasPrevious: coverage.hasPrevious })
		});
	}
	throw new TypeError('unsupported index coverage kind');
}

function assertNonNegativeSafeInteger(value: number, description: string): void {
	if (!Number.isSafeInteger(value) || value < 0) {
		throw new TypeError(`${description} must be a non-negative safe integer`);
	}
}

function assertWriterActive(active: boolean): void {
	if (!active) throw new Error('cache writer is no longer active');
}

function assertSynchronousResult(result: unknown, description: string): void {
	if (
		result !== null &&
		(typeof result === 'object' || typeof result === 'function') &&
		typeof (result as { then?: unknown }).then === 'function'
	) {
		void Promise.resolve(result).catch(() => undefined);
		throw new TypeError(`${description} must be synchronous`);
	}
}

function reportUnhandledWatcherError(error: AggregateError): void {
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
		// Observer diagnostics must never change a transaction's success semantics.
		queueMicrotask(() => {
			throw new AggregateError(
				[error, reporterError],
				'cache watcher error reporter failed'
			);
		});
	}
}

function validateRecordKeys(keys: readonly RecordKey[]): void {
	const seen = new Set<RecordKey>();
	for (const key of keys) {
		validateRecordKey(key);
		if (seen.has(key)) throw new TypeError(`duplicate record in index: ${key}`);
		seen.add(key);
	}
}

function assertName(value: string, description: string): void {
	if (typeof value !== 'string' || value.length === 0) {
		throw new TypeError(`${description} must be a non-empty string`);
	}
}

function revisionToken(value: Revision): bigint {
	if (typeof value === 'bigint') {
		if (value < 0n) throw new TypeError('revision must be an unsigned integer');
		return value;
	}
	if (typeof value === 'number') {
		if (!Number.isSafeInteger(value) || value < 0) {
			throw new TypeError('numeric revision must be an unsigned safe integer');
		}
		return BigInt(value);
	}
	if (!/^(0|[1-9][0-9]*)$/.test(value)) {
		throw new TypeError('string revision must be a canonical unsigned integer');
	}
	return BigInt(value);
}

function compareRecordTuple(
	leftIncarnation: bigint,
	leftRevision: bigint,
	rightIncarnation: bigint,
	rightRevision: bigint
): -1 | 0 | 1 {
	if (leftIncarnation < rightIncarnation) return -1;
	if (leftIncarnation > rightIncarnation) return 1;
	if (leftRevision < rightRevision) return -1;
	if (leftRevision > rightRevision) return 1;
	return 0;
}

function revisionString(value: bigint): string {
	return value.toString(10);
}

function cloneFields(
	fields: Readonly<Record<string, CacheValue>> | undefined
): Readonly<Record<string, CacheValue>> {
	if (fields === undefined) return Object.freeze({});
	return freezeRecord(
		Object.entries(fields).map(([name, value]) => {
			assertName(name, 'record field');
			return [name, cloneCacheValue(value)];
		})
	);
}

function cloneLinks(
	links: Readonly<Record<string, RecordLink>> | undefined
): Readonly<Record<string, RecordLink>> {
	if (links === undefined) return Object.freeze({});
	return freezeRecord(
		Object.entries(links).map(([name, value]) => {
			assertName(name, 'record link');
			return [name, cloneLink(value)];
		})
	);
}

function cloneLink(value: RecordLink): RecordLink {
	if (value === null) return null;
	if (typeof value === 'string') {
		validateRecordKey(value);
		return value;
	}
	if (!Array.isArray(value)) throw new TypeError('record link must be a key, key array, or null');
	validateRecordKeys(value);
	return Object.freeze([...value]);
}

function linkKeys(value: RecordLink): readonly RecordKey[] {
	if (value === null) return [];
	return typeof value === 'string' ? [value] : value;
}

function cloneCacheValue(value: CacheValue, ancestors = new Set<object>()): CacheValue {
	if (
		value === null ||
		typeof value === 'string' ||
		typeof value === 'boolean' ||
		typeof value === 'number'
	) {
		if (typeof value === 'number' && !Number.isFinite(value)) {
			throw new TypeError('cache numbers must be finite');
		}
		return value;
	}
	if (typeof value !== 'object') {
		throw new TypeError('cache fields must contain JSON-compatible values; omit absent fields');
	}
	if (ancestors.has(value)) throw new TypeError('cache fields must not contain cycles');
	ancestors.add(value);
	let cloned: CacheValue;
	if (Array.isArray(value)) {
		cloned = Object.freeze(value.map((entry) => cloneCacheValue(entry, ancestors)));
	} else {
		const prototype = Object.getPrototypeOf(value);
		if (prototype !== Object.prototype && prototype !== null) {
			throw new TypeError('cache objects must be plain JSON objects');
		}
		cloned = freezeRecord(
			Object.entries(value).map(([key, entry]) => [key, cloneCacheValue(entry, ancestors)])
		);
	}
	ancestors.delete(value);
	return cloned;
}

function freezeRecord<T>(entries: readonly (readonly [string, T])[]): Readonly<Record<string, T>> {
	const result: Record<string, T> = {};
	for (const [key, value] of entries) {
		Object.defineProperty(result, key, {
			value,
			enumerable: true,
			configurable: false,
			writable: false
		});
	}
	return Object.freeze(result);
}

function canonicalValue(value: CacheValue): string {
	if (value === null || typeof value !== 'object') return JSON.stringify(value);
	if (Array.isArray(value)) return `[${value.map(canonicalValue).join(',')}]`;
	const record = value as Readonly<Record<string, CacheValue>>;
	return `{${Object.keys(record)
		.sort()
		.map((key) => `${JSON.stringify(key)}:${canonicalValue(record[key]!)}`)
		.join(',')}}`;
}

function deepEqual(left: unknown, right: unknown): boolean {
	if (Object.is(left, right)) return true;
	if (typeof left !== typeof right || left === null || right === null) return false;
	if (typeof left !== 'object' || typeof right !== 'object') return false;
	if (Array.isArray(left) || Array.isArray(right)) {
		if (!Array.isArray(left) || !Array.isArray(right) || left.length !== right.length) return false;
		return left.every((value, index) => deepEqual(value, right[index]));
	}
	const leftRecord = left as Record<string, unknown>;
	const rightRecord = right as Record<string, unknown>;
	const leftKeys = Object.keys(leftRecord);
	const rightKeys = Object.keys(rightRecord);
	if (leftKeys.length !== rightKeys.length) return false;
	return leftKeys.every(
		(key) => Object.prototype.hasOwnProperty.call(rightRecord, key) && deepEqual(leftRecord[key], rightRecord[key])
	);
}
