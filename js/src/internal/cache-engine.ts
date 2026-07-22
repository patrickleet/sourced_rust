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
	/** Omitted fields stay absent. A present `null` remains present. */
	fields?: Readonly<Record<string, CacheValue>>;
	/** Relationship identities are stored separately from scalar/JSON fields. */
	links?: Readonly<Record<string, RecordLink>>;
};

export type OptimisticRecordWrite = Omit<RecordWrite, 'revision'>;

export type IndexWrite = {
	key: IndexKey;
	revision: Revision;
	records: readonly RecordKey[];
	complete?: boolean;
};

export type OptimisticIndexWrite = Omit<IndexWrite, 'revision'>;

export type SparseRecord = {
	readonly key: RecordKey;
	readonly revision: string;
	readonly fields: Readonly<Record<string, CacheValue>>;
	readonly links: Readonly<Record<string, RecordLink>>;
};

export type CacheIndex = {
	readonly key: IndexKey;
	readonly revision: string;
	readonly records: readonly RecordKey[];
	readonly complete: boolean;
};

export interface CacheReader {
	record(key: RecordKey): SparseRecord | undefined;
	index(key: IndexKey): CacheIndex | undefined;
}

export interface BaseCacheWriter {
	writeRecord(write: RecordWrite): boolean;
	tombstoneRecord(key: RecordKey, revision: Revision): boolean;
	writeIndex(write: IndexWrite): boolean;
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

export type OptimisticLayerState = 'optimistic' | 'accepted';

export type CacheEngineSnapshot = {
	readonly version: 1;
	readonly records: readonly {
		readonly key: RecordKey;
		readonly revision: string;
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
		readonly records: readonly RecordKey[];
		readonly complete: boolean;
		readonly deleted: boolean;
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
	rejectOptimisticLayer(id: string): boolean;
	optimisticLayerState(id: string): OptimisticLayerState | undefined;
	extract(): CacheEngineSnapshot;
	restore(snapshot: CacheEngineSnapshot): void;
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
	tombstoneRevision?: bigint;
	fields: Map<string, StoredField<CacheValue>>;
	links: Map<string, StoredField<RecordLink>>;
};

type StoredIndex = {
	revision: bigint;
	records: RecordKey[];
	complete: boolean;
	deleted: boolean;
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
	tombstoned: boolean;
	fields: Map<string, CacheValue>;
	links: Map<string, RecordLink>;
};

type VisibleIndex = {
	revision: bigint;
	records: RecordKey[];
	complete: boolean;
	deleted: boolean;
};

type Watcher<T = unknown> = {
	selector: CacheSelector<T>;
	listener: CacheListener<T>;
	value: T;
};

type EngineBackup = {
	records: Map<RecordKey, StoredRecord>;
	indexes: Map<IndexKey, StoredIndex>;
	layers: OptimisticLayer[];
	confirmedFloors: Map<string, number>;
	retained: Map<RecordKey, number>;
	nextLayerSequence: number;
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
	#dirty = false;

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
		const value = this.read(selector);
		const watcher: Watcher<T> = { selector, listener, value };
		this.#watchers.add(watcher as Watcher);
		if (options.immediate) listener(value, undefined);
		return () => this.#watchers.delete(watcher as Watcher);
	}

	batch<T>(update: (writer: BaseCacheWriter) => T): T {
		if (typeof update !== 'function') throw new TypeError('cache update must be a function');
		return this.#transaction(() => update(this.#baseWriter()));
	}

	createOptimisticLayer(
		id: string,
		update: (writer: OptimisticCacheWriter) => void
	): void {
		assertName(id, 'optimistic layer id');
		if (this.#layers.some((layer) => layer.id === id)) {
			throw new Error(`optimistic layer already exists: ${id}`);
		}

		const operations: OverlayOperation[] = [];
		const writer = this.#optimisticWriter(operations);
		update(writer);
		this.#transaction(() => {
			this.#layers.push({
				id,
				sequence: ++this.#nextLayerSequence,
				state: 'optimistic',
				operations
			});
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
		const layer = this.#layers.find((candidate) => candidate.id === id);
		if (!layer) throw new Error(`unknown optimistic layer: ${id}`);

		return this.#transaction(() => {
			const touched = new Set<string>();
			for (const operation of layer.operations) {
				for (const dependency of operationDependencies(operation)) touched.add(dependency);
			}

			const result = update(this.#baseWriter(touched));
			for (const dependency of touched) {
				const previous = this.#confirmedFloors.get(dependency) ?? 0;
				if (layer.sequence > previous) this.#confirmedFloors.set(dependency, layer.sequence);
			}
			this.#layers = this.#layers.filter((candidate) => candidate !== layer);
			if (this.#layers.length === 0) this.#confirmedFloors.clear();
			this.#dirty = true;
			return result;
		});
	}

	rejectOptimisticLayer(id: string): boolean {
		const layer = this.#layers.find((candidate) => candidate.id === id);
		if (!layer) return false;
		this.#transaction(() => {
			this.#layers = this.#layers.filter((candidate) => candidate !== layer);
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
							records: Object.freeze([...index.records]),
							complete: index.complete,
							deleted: index.deleted
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
			this.#dirty = true;
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
			const reachable = new Set<RecordKey>([
				...this.#reachableRecords(this.#materialize(false), optimisticRoots),
				...this.#reachableRecords(this.#materialize(true), optimisticRoots)
			]);

			const collected: RecordKey[] = [];
			for (const [key, record] of this.#records) {
				// Tombstones are revision fences and cannot be collected like data rows.
				if (record.tombstoneRevision === undefined && !reachable.has(key)) {
					this.#records.delete(key);
					collected.push(key);
				}
			}
			if (collected.length > 0) this.#dirty = true;
			return Object.freeze(collected.sort());
		});
	}

	#reader(): CacheReader {
		const { records, indexes } = this.#materialize();
		return Object.freeze({
			record(key: RecordKey): SparseRecord | undefined {
				const record = records.get(key);
				if (!record || record.tombstoned) return undefined;
				return Object.freeze({
					key,
					revision: revisionString(record.revision),
					fields: freezeRecord(
						[...record.fields].map(([name, value]) => [name, cloneCacheValue(value)])
					),
					links: freezeRecord(
						[...record.links].map(([name, value]) => [name, cloneLink(value)])
					)
				});
			},
			index(key: IndexKey): CacheIndex | undefined {
				const index = indexes.get(key);
				if (!index || index.deleted) return undefined;
				return Object.freeze({
					key,
					revision: revisionString(index.revision),
					records: Object.freeze([...index.records]),
					complete: index.complete
				});
			}
		});
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
		for (const index of graph.indexes.values()) {
			if (index.deleted) continue;
			for (const key of index.records) {
				if (reachable.has(key)) continue;
				reachable.add(key);
				pending.push(key);
			}
		}
		while (pending.length > 0) {
			const record = graph.records.get(pending.pop()!);
			if (!record || record.tombstoned) continue;
			for (const link of record.links.values()) {
				for (const key of linkKeys(link)) {
					if (reachable.has(key)) continue;
					reachable.add(key);
					pending.push(key);
				}
			}
		}
		return reachable;
	}

	#materialize(includeOptimistic = true): {
		records: Map<RecordKey, VisibleRecord>;
		indexes: Map<IndexKey, VisibleIndex>;
	} {
		const records = new Map<RecordKey, VisibleRecord>();
		const indexes = new Map<IndexKey, VisibleIndex>();

		for (const [key, record] of this.#records) {
			records.set(key, {
				revision: record.revision,
				tombstoned: record.tombstoneRevision !== undefined,
				fields: new Map([...record.fields].map(([name, field]) => [name, field.value])),
				links: new Map([...record.links].map(([name, link]) => [name, link.value]))
			});
		}
		for (const [key, index] of this.#indexes) {
			indexes.set(key, {
				revision: index.revision,
				records: [...index.records],
				complete: index.complete,
				deleted: index.deleted
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
			indexes.set(key, {
				revision: indexes.get(key)?.revision ?? 0n,
				records: [...operation.write.records],
				complete: operation.write.complete ?? false,
				deleted: false
			});
		} else {
			const index = indexes.get(key) ?? {
				revision: 0n,
				records: [],
				complete: false,
				deleted: true
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

	#baseWriter(touched?: Set<string>): BaseCacheWriter {
		return Object.freeze({
			writeRecord: (write: RecordWrite) => this.#writeBaseRecord(write, touched),
			tombstoneRecord: (key: RecordKey, revision: Revision) =>
				this.#tombstoneBaseRecord(key, revision, touched),
			writeIndex: (write: IndexWrite) => this.#writeBaseIndex(write, touched),
			deleteIndex: (key: IndexKey, revision: Revision) =>
				this.#deleteBaseIndex(key, revision, touched)
		});
	}

	#optimisticWriter(operations: OverlayOperation[]): OptimisticCacheWriter {
		return Object.freeze({
			writeRecord(write: OptimisticRecordWrite): void {
				validateRecordKey(write.key);
				const fields = cloneFields(write.fields);
				const links = cloneLinks(write.links);
				if (Object.keys(fields).length === 0 && Object.keys(links).length === 0) return;
				operations.push({ kind: 'write-record', write: { key: write.key, fields, links } });
			},
			tombstoneRecord(key: RecordKey): void {
				validateRecordKey(key);
				operations.push({ kind: 'tombstone-record', key });
			},
			writeIndex(write: OptimisticIndexWrite): void {
				validateIndexWrite(write);
				operations.push({
					kind: 'write-index',
					write: {
						key: write.key,
						records: Object.freeze([...write.records]),
						complete: write.complete ?? false
					}
				});
			},
			deleteIndex(key: IndexKey): void {
				assertName(key, 'index key');
				operations.push({ kind: 'delete-index', key });
			}
		});
	}

	#writeBaseRecord(write: RecordWrite, touched?: Set<string>): boolean {
		validateRecordKey(write.key);
		const revision = revisionToken(write.revision);
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
		if (record?.tombstoneRevision !== undefined && revision <= record.tombstoneRevision) {
			return false;
		}
		if (record && revision < record.revision) return false;

		let changed = false;
		if (!record) {
			record = { revision, fields: new Map(), links: new Map() };
			this.#records.set(write.key, record);
			changed = true;
		} else if (record.tombstoneRevision !== undefined) {
			record.fields.clear();
			record.links.clear();
			record.tombstoneRevision = undefined;
			changed = true;
		}

		for (const [name, value] of Object.entries(fields)) {
			const current = record.fields.get(name);
			if (current && revision < current.revision) continue;
			if (current && revision === current.revision) {
				if (deepEqual(current.value, value)) continue;
				// Conflicting data at one source revision is not allowed to race by arrival order.
				continue;
			}
			record.fields.set(name, { revision, value });
			changed = true;
		}
		for (const [name, value] of Object.entries(links)) {
			const current = record.links.get(name);
			if (current && revision < current.revision) continue;
			if (current && revision === current.revision) {
				if (deepEqual(current.value, value)) continue;
				continue;
			}
			record.links.set(name, { revision, value });
			changed = true;
		}
		if (revision > record.revision) {
			record.revision = revision;
			changed = true;
		}
		if (changed) this.#dirty = true;
		return changed;
	}

	#tombstoneBaseRecord(
		key: RecordKey,
		revisionValue: Revision,
		touched?: Set<string>
	): boolean {
		validateRecordKey(key);
		const revision = revisionToken(revisionValue);
		touched?.add(recordWildcardDependency(key));
		touched?.add(recordSeenDependency(key));
		const record = this.#records.get(key);
		if (record) {
			if (record.tombstoneRevision !== undefined && revision <= record.tombstoneRevision) {
				return false;
			}
			if (record.tombstoneRevision === undefined && revision <= record.revision) return false;
			record.revision = revision;
			record.tombstoneRevision = revision;
			record.fields.clear();
			record.links.clear();
		} else {
			this.#records.set(key, {
				revision,
				tombstoneRevision: revision,
				fields: new Map(),
				links: new Map()
			});
		}
		this.#dirty = true;
		return true;
	}

	#writeBaseIndex(write: IndexWrite, touched?: Set<string>): boolean {
		validateIndexWrite(write);
		const revision = revisionToken(write.revision);
		touched?.add(indexDependency(write.key));
		const current = this.#indexes.get(write.key);
		if (current && revision < current.revision) return false;
		const records = [...write.records];
		const complete = write.complete ?? false;
		if (current && revision === current.revision) {
			if (
				!current.deleted &&
				current.complete === complete &&
				deepEqual(current.records, records)
			) {
				return false;
			}
			return false;
		}
		this.#indexes.set(write.key, {
			revision,
			records,
			complete,
			deleted: false
		});
		this.#dirty = true;
		return true;
	}

	#deleteBaseIndex(key: IndexKey, revisionValue: Revision, touched?: Set<string>): boolean {
		assertName(key, 'index key');
		const revision = revisionToken(revisionValue);
		touched?.add(indexDependency(key));
		const current = this.#indexes.get(key);
		if (current && revision <= current.revision) return false;
		this.#indexes.set(key, {
			revision,
			records: [],
			complete: false,
			deleted: true
		});
		this.#dirty = true;
		return true;
	}

	#transaction<T>(update: () => T): T {
		const outermost = this.#transactionDepth === 0;
		const backup = outermost ? this.#backup() : undefined;
		this.#transactionDepth += 1;
		let result: T;
		try {
			result = update();
		} catch (error) {
			this.#transactionDepth -= 1;
			if (outermost && backup) {
				this.#restoreBackup(backup);
				this.#dirty = false;
			}
			throw error;
		}
		this.#transactionDepth -= 1;
		if (outermost) {
			const shouldFlush = this.#dirty;
			this.#dirty = false;
			if (shouldFlush) this.#flushWatchers();
		}
		return result;
	}

	#flushWatchers(): void {
		for (const watcher of this.#watchers) {
			const next = this.read(watcher.selector);
			if (deepEqual(next, watcher.value)) continue;
			const previous = watcher.value;
			watcher.value = next;
			watcher.listener(next, previous);
		}
	}

	#backup(): EngineBackup {
		return structuredClone({
			records: this.#records,
			indexes: this.#indexes,
			layers: this.#layers,
			confirmedFloors: this.#confirmedFloors,
			retained: this.#retained,
			nextLayerSequence: this.#nextLayerSequence
		});
	}

	#restoreBackup(backup: EngineBackup): void {
		this.#records = backup.records;
		this.#indexes = backup.indexes;
		this.#layers = backup.layers;
		this.#confirmedFloors = backup.confirmedFloors;
		this.#retained = backup.retained;
		this.#nextLayerSequence = backup.nextLayerSequence;
	}
}

/** Create the selected private cache-engine implementation. */
export function createCacheEngine(): CacheEngine {
	return new PurposeBuiltCacheEngine();
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
		records.set(input.key, { revision, tombstoneRevision, fields, links });
	}

	const indexes = new Map<IndexKey, StoredIndex>();
	for (const input of snapshot.indexes) {
		assertName(input.key, 'index key');
		if (indexes.has(input.key)) throw new TypeError(`duplicate snapshot index: ${input.key}`);
		validateRecordKeys(input.records);
		indexes.set(input.key, {
			revision: revisionToken(input.revision),
			records: [...input.records],
			complete: Boolean(input.complete),
			deleted: Boolean(input.deleted)
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

function emptyVisibleRecord(): VisibleRecord {
	return { revision: 0n, tombstoned: false, fields: new Map(), links: new Map() };
}

function validateRecordKey(key: RecordKey): void {
	assertName(key, 'record key');
}

function validateIndexWrite(write: { key: IndexKey; records: readonly RecordKey[] }): void {
	assertName(write.key, 'index key');
	if (!Array.isArray(write.records)) throw new TypeError('index records must be an array');
	validateRecordKeys(write.records);
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
	const result: Record<string, T> = Object.create(null) as Record<string, T>;
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
